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

#[cfg(test)]
thread_local! {
    /// How many of this thread's next compressions fail: the failure path's units.
    static FAILS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// How a commit ended, for the opcode posted after the lock is released.
enum Ended {
    Added { enable_sending: bool },
    Overflow,
    CompressionFailed,
}

/// What became of a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Committed {
    /// A session's buffer took it.
    Taken,
    /// The buffer was flushed since the caller's flush time (D105.6).
    Flushed,
    /// Dropped: nothing to commit, no session, or a buffer that overflowed or failed to compress (its session restarts).
    Dropped,
}

/// `nd_log_limit_static_global_var(erl, 1, 0)`: one record a second, for all hosts.
static OVERFLOW_RECORD: ErrorLimit = ErrorLimit::new(1, 0);
static COMPRESSION_RECORD: ErrorLimit = ErrorLimit::new(1, 0);

impl Sender {
    /// `sender_buffer_commit()`: dropped without a dispatched connection (no session).
    pub fn commit(&self, src: &[u8], traffic: Traffic) {
        self.commit_into(src, traffic, None);
    }

    /// A replication answer's commit, only into the session whose buffer flush it was asked after (D105.6; C commits
    /// it into a newer session too): whether it counts as sent, for the host's counters and the chart's finish. It
    /// does unless the buffer was flushed since; without a session (the reconnect's delay) or with a session being
    /// restarted, C drops the bytes and still counts it (`stream-replication-sender.c:697-716`, R55 M5).
    pub(crate) fn commit_replication(&self, src: &[u8], flush_ut: u64) -> bool {
        self.commit_checked(src, Traffic::Replication, Some(flush_ut)) != Committed::Flushed
    }

    /// The commit, when the buffer was last flushed at `flush_ut` if one is given; whether a session took it.
    pub(crate) fn commit_into(&self, src: &[u8], traffic: Traffic, flush_ut: Option<u64>) -> bool {
        self.commit_checked(src, traffic, flush_ut) == Committed::Taken
    }

    fn commit_checked(&self, src: &[u8], traffic: Traffic, flush_ut: Option<u64>) -> Committed {
        if src.is_empty() {
            return Committed::Dropped;
        }
        // only the rare records name the host: no lookup on the hot path
        let hostname = || self.hostname();
        let mut out = self.out();
        if flush_ut.is_some_and(|f| out.buffer.last_flush_ut() != f) {
            return Committed::Flushed;
        }
        // "the dispatcher is not there anymore - ignore these data"
        let Some(session) = out.session else {
            return Committed::Dropped;
        };
        if out.buffer.set_max_size(src.len() * ADAPT_TO_TIMES_MAX_SIZE, false) {
            nd_log!(
                Source::Daemon,
                Priority::Notice,
                "STREAM SND '{}' [to {}]: Increased max buffer size to {} (message size {}).",
                hostname(),
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
                self.connector.replication().recalculate(&self.replication, out.buffer.used_percent());
                drop(out);
                if enable_sending {
                    self.post(session, op::POLLOUT, Reason::NEVER);
                }
                Committed::Taken
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
                    "STREAM SND '{}' [to {remote_ip}]: buffer overflow (buffer size {}, max size {}, available {}). \
                     Restarting connection.",
                    hostname(),
                    stats.bytes_size,
                    stats.bytes_max_size,
                    stats.bytes_available
                );
                Committed::Dropped
            }
            Ended::CompressionFailed => {
                self.deactivate_compression(&out, &hostname());
                let remote_ip = out.remote_ip.clone();
                drop(out);
                self.post(session, op::RECONNECT_WITHOUT_COMPRESSION, Reason::SND_DISCONNECT_COMPRESSION_FAILED);
                nd_log_limit!(
                    &COMPRESSION_RECORD,
                    Source::Daemon,
                    Priority::Err,
                    "STREAM SND '{}' [to {remote_ip}]: COMPRESSION failed (twice). Deactivating compression and \
                     restarting connection.",
                    hostname()
                );
                Committed::Dropped
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
fn compressed(out: &mut Out, hostname: &dyn Fn() -> String, src: &[u8], traffic: Traffic, enable_sending: bool) -> Ended {
    let binary = out.capabilities & caps::BINARY != 0;
    let mut rest = src;
    while !rest.is_empty() {
        let piece = &rest[..next_piece(rest, binary)];
        let compress = |out: &mut Out| {
            #[cfg(test)]
            if FAILS.with(|f| f.get().checked_sub(1).inspect(|&n| f.set(n)).is_some()) {
                return None;
            }
            out.compressor.as_mut().and_then(|c| c.compress(piece)).map(<[u8]>::len)
        };
        let mut len = compress(out);
        if len.is_none() {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM SND '{}' [to {}]: COMPRESSION failed. Resetting compressor and re-trying",
                hostname(),
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use netdata_agent_evloop::Pool;
    use netdata_agent_log::{Priority, capture};
    use netdata_agent_rrd::host::Host;

    use super::*;
    use crate::compression::decode_signature;
    use crate::connector::Connector;
    use crate::connector::tests::{connector, info};
    use crate::decompress::Decompressor;
    use crate::sender::{Ops, Session};
    use crate::thread::StreamMsg;

    /// A sender dispatched with `capabilities` and their compressor, its opcodes gathered in its slot (no message to a
    /// stream thread).
    fn dispatched(capabilities: u32, n: u8) -> (Pool<StreamMsg>, Arc<Connector>, Arc<Host>, Arc<Sender>, Session) {
        let (pool, c) = connector();
        let host = Arc::new(Host::new(
            &format!("5a1e0000-0000-4000-8000-0000000000{n:02x}"),
            false,
            info("127.0.0.1:1", "key"),
        ));
        let s = Sender::attach(&host, &c).expect("created");
        let session = Session { thread: 9, id: 1 };
        {
            let mut out = s.out();
            out.session = Some(session);
            out.remote_ip = "parent".into();
            out.capabilities = capabilities;
            out.algorithm = Algorithm::for_capabilities(capabilities);
            let levels = out.levels;
            out.compressor = out.algorithm.and_then(|a| Compressor::new(a, &levels));
        }
        *s.ops.lock().unwrap() = Some(Ops { session, bits: 0, reason: Reason::NEVER });
        (pool, c, host, s, session)
    }

    /// The messages in the sender's buffer, decompressed one at a time as the parent does.
    fn pieces(bytes: &[u8], capabilities: u32) -> Vec<Vec<u8>> {
        let mut rest = bytes;
        let mut all = Vec::new();
        let mut d = Decompressor::for_capabilities(capabilities).unwrap();
        while !rest.is_empty() {
            let len = decode_signature([rest[0], rest[1], rest[2], rest[3]]).unwrap();
            d.feed(&rest[..4 + len]);
            let mut plain = Vec::new();
            assert_eq!(d.next_message(&mut plain), Ok(true));
            all.push(plain);
            rest = &rest[4 + len..];
        }
        all
    }

    fn texts(records: Vec<netdata_agent_log::Captured>) -> Vec<(Priority, String)> {
        records.into_iter().map(|r| (r.priority, r.message.unwrap_or_default())).collect()
    }

    /// `sender_buffer_commit()`'s loop: a commit is compressed in pieces cut at the last newline before 16255 bytes,
    /// at 16255 when there is none or only the first byte is one, and the rest whole; with BINARY at 16255 always.
    /// Every piece is one message after its signature, counted with its plain size.
    #[test]
    fn a_commit_is_compressed_in_c_s_pieces() {
        let mut src = Vec::new();
        for (byte, n) in [(b'a', 9999), (b'\n', 1), (b'b', 9999), (b'\n', 1), (b'c', 16255), (b'\n', 1), (b'd', 16300), (b'e', 100), (b'\n', 1)] {
            src.extend(std::iter::repeat_n(byte, n));
        }
        assert_eq!(src.len(), 52_657);
        for (n, binary, want) in [
            (0xe1, 0, vec![10_000, 10_000, 16_255, 16_255, 147]),
            (0xe2, caps::BINARY, vec![16_255, 16_255, 16_255, 3_892]),
        ] {
            let (_pool, _c, _host, s, session) = dispatched(caps::ZSTD | binary, n);
            s.commit(&src, Traffic::Data);
            let out = s.out();
            let got = pieces(out.buffer.next(), caps::ZSTD);
            assert_eq!(got.iter().map(Vec::len).collect::<Vec<_>>(), want, "binary {binary}");
            assert_eq!(got.concat(), src);
            let stats = out.buffer.stats();
            assert_eq!((stats.adds, stats.bytes_uncompressed), (2 * want.len(), src.len() + 4 * want.len()));
            drop(out);
            let ops = s.take_ops(session).unwrap();
            assert_eq!((ops.bits, ops.reason), (op::POLLOUT, Reason::NEVER));
        }
    }

    /// A failed compression sets the compressor up again and retries the piece once; a second failure disables the
    /// algorithm for the host's next connections and restarts this one without compression (C's records, the
    /// disabled bit, RECONNECT_WITHOUT_COMPRESSION with SND COMPRESSION FAILED).
    #[test]
    fn a_failing_compressor_is_set_up_again_once_then_its_algorithm_disabled() {
        let retrying = (
            Priority::Err,
            "STREAM SND 'child' [to parent]: COMPRESSION failed. Resetting compressor and re-trying".to_string(),
        );
        // one failure: the piece goes in a new zstd frame of a compressor set up again
        let (_pool, _c, _host, s, session) = dispatched(caps::ZSTD | caps::LZ4, 0xe3);
        s.commit(b"a\n", Traffic::Data);
        FAILS.with(|f| f.set(1));
        let ((), records) = capture(|| s.commit(b"x\n", Traffic::Data));
        assert_eq!(texts(records), std::slice::from_ref(&retrying));
        let bytes = s.out().buffer.next().to_vec();
        let second = 4 + decode_signature([bytes[0], bytes[1], bytes[2], bytes[3]]).unwrap();
        assert_eq!(pieces(&bytes[..second], caps::ZSTD), [b"a\n".to_vec()]);
        assert_eq!(pieces(&bytes[second..], caps::ZSTD), [b"x\n".to_vec()]);
        assert_eq!(bytes[second + 4..second + 8], [0x28, 0xb5, 0x2f, 0xfd], "a new frame");
        assert_eq!(s.disabled.load(Ordering::Relaxed), 0);
        let ops = s.take_ops(session).unwrap();
        assert_eq!((ops.bits, ops.reason), (op::POLLOUT, Reason::NEVER));

        // two failures
        let (_pool, _c, _host, s, session) = dispatched(caps::ZSTD | caps::LZ4, 0xe4);
        FAILS.with(|f| f.set(2));
        let ((), records) = capture(|| s.commit(b"x\n", Traffic::Data));
        assert_eq!(
            texts(records),
            [
                retrying,
                (Priority::Err, "STREAM_COMPRESSION: ZSTD compression error on 'host:child'. Disabling ZSTD for this node.".to_string()),
                (Priority::Err, "STREAM SND 'child' [to parent]: COMPRESSION failed (twice). Deactivating compression and restarting connection.".to_string()),
            ]
        );
        assert_eq!(s.out().buffer.stats().adds, 0);
        assert_eq!(s.disabled.load(Ordering::Relaxed), caps::ZSTD);
        let ops = s.take_ops(session).unwrap();
        assert_eq!((ops.bits, ops.reason), (op::RECONNECT_WITHOUT_COMPRESSION, Reason::SND_DISCONNECT_COMPRESSION_FAILED));
        // the next connection offers every compression but zstd
        assert_eq!(
            caps::sender_ours(s.disabled.load(Ordering::Relaxed), 0) & caps::COMPRESSIONS_AVAILABLE,
            caps::COMPRESSIONS_AVAILABLE & !caps::ZSTD
        );

        // each algorithm's record and bit; C names ZSTD in lz4's
        for (n, cap, want) in [
            (0xe5, caps::LZ4, "STREAM_COMPRESSION: LZ4 compression error on 'host:child'. Disabling ZSTD for this node."),
            (0xe6, caps::BROTLI, "STREAM_COMPRESSION: BROTLI compression error on 'host:child'. Disabling BROTLI for this node."),
            (0xe7, caps::GZIP, "STREAM_COMPRESSION: GZIP compression error on 'host:child'. Disabling GZIP for this node."),
        ] {
            let (_pool, _c, _host, s, _) = dispatched(cap, n);
            FAILS.with(|f| f.set(2));
            let ((), records) = capture(|| s.commit(b"x\n", Traffic::Data));
            let deactivated: Vec<_> = texts(records).into_iter().filter(|(_, t)| t.starts_with("STREAM_COMPRESSION:")).collect();
            assert_eq!(deactivated, [(Priority::Err, want.to_string())]);
            assert_eq!(s.disabled.load(Ordering::Relaxed), cap);
        }
    }
}
