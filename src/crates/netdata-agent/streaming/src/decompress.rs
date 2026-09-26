//! The receiving side of stream compression (`src/streaming/stream-compression/`): each compressed message is a
//! 4-byte signature carrying its length, followed by that many bytes of one continuous zstd, lz4, brotli or gzip
//! stream per connection. Decompressed output feeds the line reader. A message that fails any of C's checks ends
//! the connection.

use brotli_decompressor::{BrotliDecompressStream, BrotliResult, BrotliState, StandardAlloc};
use flate2::{Decompress, FlushDecompress, Status};
use netdata_agent_log::{Priority, Source, nd_log};
use zstd::stream::raw::{Decoder as ZstdDecoder, InBuffer, Operation, OutBuffer};

use crate::caps;

/// The engines' own records before a failed message ends the connection (`netdata_log_error()`).
fn failed(message: std::fmt::Arguments<'_>) {
    nd_log!(
        Source::Daemon,
        Priority::Err,
        "STREAM_DECOMPRESS: {message}"
    );
}

/// `COMPRESSION_MAX_CHUNK`.
const MAX_CHUNK: usize = 0x4000;
/// `COMPRESSION_MAX_MSG_SIZE`: the largest compressed message accepted.
const MAX_MSG_SIZE: usize = MAX_CHUNK - 128 - 1;
/// `STREAM_COMPRESSION_SIGNATURE_SIZE`.
const SIGNATURE_SIZE: usize = 4;

/// The output buffer sizes C allocates (`simple_ring_buffer_make_room()` from empty: 16 KiB, then + the request).
const ZSTD_OUTPUT: usize = MAX_CHUNK + 131_072;
const LZ4_RING: usize = MAX_CHUNK + 65_536 + MAX_CHUNK * 2;
/// gzip and brotli: `simple_ring_buffer_set_capacity(COMPRESSION_MAX_CHUNK + 1)`.
const SMALL_OUTPUT: usize = MAX_CHUNK + 1;
/// How far back an lz4 block may reference.
const LZ4_WINDOW: usize = 65_536;

/// `stream_decompress_decode_signature()`: the compressed length, or `None` when the bytes are not a signature
/// (uncompressed data inside a compressed stream). The signature is a host-endian `uint32`: little-endian here.
pub fn decode_signature(bytes: [u8; SIGNATURE_SIZE]) -> Option<usize> {
    const SIGNATURE: u32 =
        (b'z' as u32 | 0x80) | (0x80 << 8) | (0x80 << 16) | ((b'\n' as u32) << 24);
    const MASK: u32 = 0xff | (0x80 << 8) | (0x80 << 16) | (0xff << 24);
    let sign = u32::from_le_bytes(bytes);
    if sign & MASK != SIGNATURE {
        return None;
    }
    Some((((sign >> 8) & 0x7f) | ((sign >> 9) & (0x7f << 7))) as usize)
}

/// One connection's decompression state (`struct decompressor_state`).
enum Engine {
    Zstd(Box<ZstdDecoder<'static>>),
    Lz4 {
        /// The last `LZ4_WINDOW` decompressed bytes: what a block may reference.
        history: Vec<u8>,
        /// Where C's ring would write next; decides the room a block may decompress into.
        write_pos: usize,
    },
    /// zlib's own gzip inflate, as C's `inflateInit2(15 + 16)`.
    Gzip(Box<Decompress>),
    Brotli(Box<BrotliState<StandardAlloc, StandardAlloc, StandardAlloc>>),
}

/// Why the connection ends; it displays as C's log message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Multiplexed,
    /// The compressed size a message announced.
    TooBig(usize),
    NoBytes,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Multiplexed => {
                f.write_str("multiplexed uncompressed data in compressed stream!")
            }
            Failure::TooBig(size) => write!(
                f,
                "received a compressed message of {size} bytes, which is bigger than the max compressed message size \
                 supported of {MAX_MSG_SIZE}. Ignoring message."
            ),
            Failure::NoBytes => f.write_str("no bytes to decompress."),
        }
    }
}

/// A connection's decompressor: frames the received bytes into messages and decompresses them in order.
pub struct Decompressor {
    engine: Engine,
    /// Received bytes not yet part of a complete message.
    pending: Vec<u8>,
    output: Vec<u8>,
}

impl Decompressor {
    /// `stream_decompression_initialize()`: the negotiated compression, by the fixed priority zstd, lz4, brotli,
    /// gzip; `None` for an uncompressed stream.
    pub fn for_capabilities(capabilities: u32) -> Option<Self> {
        let (engine, output) = if capabilities & caps::ZSTD != 0 {
            (
                Engine::Zstd(Box::new(ZstdDecoder::new().ok()?)),
                ZSTD_OUTPUT,
            )
        } else if capabilities & caps::LZ4 != 0 {
            (
                Engine::Lz4 {
                    history: Vec::new(),
                    write_pos: 0,
                },
                LZ4_RING,
            )
        } else if capabilities & caps::BROTLI != 0 {
            let state = BrotliState::new(
                StandardAlloc::default(),
                StandardAlloc::default(),
                StandardAlloc::default(),
            );
            (Engine::Brotli(Box::new(state)), SMALL_OUTPUT)
        } else if capabilities & caps::GZIP != 0 {
            (
                Engine::Gzip(Box::new(Decompress::new_gzip(15))),
                SMALL_OUTPUT,
            )
        } else {
            return None;
        };
        Some(Decompressor {
            engine,
            pending: Vec::new(),
            output: vec![0; output],
        })
    }

    /// `receiver_feed_decompressor()` over everything received: appends every complete message's decompressed bytes
    /// to `out` and keeps an incomplete one for later.
    pub fn push(&mut self, received: &[u8], out: &mut Vec<u8>) -> Result<(), Failure> {
        self.pending.extend_from_slice(received);
        let mut start = 0;
        let result = loop {
            let Some(header) = self.pending.get(start..start + SIGNATURE_SIZE) else {
                break Ok(());
            };
            let Some(size) =
                decode_signature([header[0], header[1], header[2], header[3]]).filter(|&s| s != 0)
            else {
                break Err(Failure::Multiplexed);
            };
            if size > MAX_MSG_SIZE {
                break Err(Failure::TooBig(size));
            }
            let body = start + SIGNATURE_SIZE;
            if body + size > self.pending.len() {
                break Ok(());
            }
            let message = self.pending[body..body + size].to_vec();
            match self.decompress(&message) {
                Ok(0) | Err(_) => break Err(Failure::NoBytes),
                Ok(n) => out.extend_from_slice(&self.output[..n]),
            }
            start = body + size;
        };
        self.pending.drain(..start);
        result
    }

    /// `stream_decompress()`: one message into the output buffer; the decompressed length (0 is a failure too). A
    /// failing engine logs C's line first.
    fn decompress(&mut self, data: &[u8]) -> Result<usize, ()> {
        let output = &mut self.output;
        let size = output.len();
        match &mut self.engine {
            Engine::Zstd(decoder) => {
                let mut input = InBuffer::around(data);
                let mut out = OutBuffer::around(&mut output[..]);
                // the crate's error text is ZSTD_getErrorName()
                if let Err(e) = decoder.run(&mut input, &mut out) {
                    failed(format_args!("ZSTD_decompressStream() return error: {e}"));
                    return Err(());
                }
                // A frame decompressing to more than the buffer is refused.
                if input.pos < data.len() {
                    failed(format_args!(
                        "ZSTD_decompressStream() consumed only {} of {} compressed bytes after filling the {size}-byte \
                         output buffer (frame decompresses to more than the buffer); failing the connection",
                        input.pos,
                        data.len()
                    ));
                    return Err(());
                }
                Ok(out.pos())
            }
            Engine::Lz4 { history, write_pos } => {
                if *write_pos + MAX_CHUNK > LZ4_RING {
                    *write_pos = 0;
                }
                let room = LZ4_RING - *write_pos;
                match lz4_flex::block::decompress_into_with_dict(data, &mut output[..room], history)
                {
                    Ok(n) => {
                        *write_pos += n;
                        history.extend_from_slice(&output[..n]);
                        if history.len() > LZ4_WINDOW {
                            history.drain(..history.len() - LZ4_WINDOW);
                        }
                        Ok(n)
                    }
                    Err(_) => {
                        // liblz4 returns where in the input it failed, which lz4_flex does not tell (D55)
                        failed(format_args!(
                            "LZ4_decompress_safe_continue() returned negative value: -1 (compressed chunk is {} bytes)",
                            data.len()
                        ));
                        Err(())
                    }
                }
            }
            Engine::Gzip(inflate) => {
                let (taken, produced) = (inflate.total_in(), inflate.total_out());
                // zlib's return codes as C prints them: Z_NEED_DICT, Z_DATA_ERROR, Z_BUF_ERROR
                let code = match inflate.decompress(data, &mut output[..], FlushDecompress::Sync) {
                    Ok(Status::Ok | Status::StreamEnd) => 0,
                    Ok(Status::BufError) => -5,
                    Err(e) if e.needs_dictionary().is_some() => 2,
                    Err(_) => -3,
                };
                if code != 0 {
                    failed(format_args!("inflate() failed with error {code}"));
                    return Err(());
                }
                let remaining = data.len() - (inflate.total_in() - taken) as usize;
                if remaining != 0 {
                    failed(format_args!(
                        "inflate() did not use all compressed data we provided (compressed payload {} bytes, \
                         remaining to be uncompressed {remaining})",
                        data.len()
                    ));
                    return Err(());
                }
                let produced = (inflate.total_out() - produced) as usize;
                if produced == size {
                    failed(format_args!(
                        "inflate() produced at least {size} bytes, exceeding the max supported size of {MAX_CHUNK} \
                         bytes (compressed payload {} bytes)",
                        data.len()
                    ));
                    return Err(());
                }
                Ok(produced)
            }
            Engine::Brotli(state) => {
                let (mut available_in, mut input_offset) = (data.len(), 0);
                let (mut available_out, mut output_offset, mut total_out) = (size, 0, 0);
                let result = BrotliDecompressStream(
                    &mut available_in,
                    &mut input_offset,
                    data,
                    &mut available_out,
                    &mut output_offset,
                    &mut output[..],
                    &mut total_out,
                    state,
                );
                if matches!(result, BrotliResult::ResultFailure) {
                    failed(format_args!("Brotli decompression failed."));
                    return Err(());
                }
                if available_in != 0 {
                    failed(format_args!(
                        "BrotliDecoderDecompressStream() did not use all the input buffer, {available_in} bytes out \
                         of {} remain",
                        data.len()
                    ));
                    return Err(());
                }
                if available_out == 0 {
                    failed(format_args!(
                        "BrotliDecoderDecompressStream() produced at least {size} bytes, exceeding the max supported \
                         size of {MAX_CHUNK} bytes (compressed payload {} bytes)",
                        data.len()
                    ));
                    return Err(());
                }
                let produced = size - available_out;
                if produced == 0 {
                    failed(format_args!(
                        "BrotliDecoderDecompressStream() did not produce any output from the input provided (input \
                         buffer {} bytes)",
                        data.len()
                    ));
                    return Err(());
                }
                Ok(produced)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// `stream_compress_encode_signature()`.
    fn frame(payload: &[u8]) -> Vec<u8> {
        let len = payload.len() as u32;
        let signature = (((len & 0x7f) | 0x80 | (((len & (0x7f << 7)) << 1) | 0x8000)) << 8)
            | 0xfa
            | ((b'\n' as u32) << 24);
        let mut out = signature.to_le_bytes().to_vec();
        out.extend_from_slice(payload);
        out
    }

    fn lines(n: usize) -> Vec<Vec<u8>> {
        (0..n)
            .map(|i| {
                format!(
                    "BEGIN2 'chart.{}' 1 {} #\nSET2 'd' {i} {i} A\nEND2\n",
                    i % 7,
                    1_700_000_000 + i
                )
                .into_bytes()
            })
            .collect()
    }

    /// Decompresses `stream` fed in pieces of `step` bytes.
    fn run(capabilities: u32, stream: &[u8], step: usize) -> Result<Vec<u8>, Failure> {
        let mut d = Decompressor::for_capabilities(capabilities).unwrap();
        let mut out = Vec::new();
        for piece in stream.chunks(step) {
            d.push(piece, &mut out)?;
        }
        Ok(out)
    }

    #[test]
    fn signatures_round_trip() {
        for len in [1usize, 127, 128, 5000, 16383] {
            let f = frame(&vec![0; len]);
            assert_eq!(decode_signature([f[0], f[1], f[2], f[3]]), Some(len));
        }
        assert_eq!(decode_signature(*b"BEGI"), None);
    }

    #[test]
    fn zstd_streams_decompress_in_any_pieces() {
        let chunks = lines(200);
        let mut encoder = zstd::stream::raw::Encoder::new(3).unwrap();
        let mut stream = Vec::new();
        for chunk in &chunks {
            let mut buf = vec![0u8; 32768];
            let mut out = OutBuffer::around(&mut buf[..]);
            let mut input = InBuffer::around(chunk);
            encoder.run(&mut input, &mut out).unwrap();
            encoder.flush(&mut out).unwrap();
            let n = out.pos();
            stream.extend(frame(&buf[..n]));
        }
        let plain: Vec<u8> = chunks.concat();
        for step in [1, 7, 4096, stream.len()] {
            assert_eq!(run(caps::ZSTD, &stream, step).unwrap(), plain);
        }
    }

    #[test]
    fn lz4_blocks_reference_earlier_ones() {
        let chunks = lines(8000);
        let mut stream = Vec::new();
        let mut history: Vec<u8> = Vec::new();
        for chunk in chunks.chunks(20).map(|c| c.concat()) {
            let dict = &history[history.len().saturating_sub(LZ4_WINDOW)..];
            let block = lz4_flex::block::compress_with_dict(&chunk, dict);
            stream.extend(frame(&block));
            history.extend_from_slice(&chunk);
        }
        assert!(history.len() > 2 * LZ4_RING, "the ring wraps");
        assert_eq!(run(caps::LZ4, &stream, 1000).unwrap(), history);
    }

    #[test]
    fn gzip_sync_flushed_members_decompress() {
        let chunks = lines(100);
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut stream = Vec::new();
        let mut sent = 0;
        for chunk in &chunks {
            encoder.write_all(chunk).unwrap();
            encoder.flush().unwrap();
            let all = encoder.get_ref();
            stream.extend(frame(&all[sent..]));
            sent = all.len();
        }
        assert_eq!(run(caps::GZIP, &stream, 3).unwrap(), chunks.concat());
    }

    #[test]
    fn a_too_big_message_is_logged_with_c_s_sizes() {
        assert_eq!(
            Failure::TooBig(MAX_MSG_SIZE + 1).to_string(),
            format!(
                "received a compressed message of {} bytes, which is bigger than the max compressed message size \
                 supported of {MAX_MSG_SIZE}. Ignoring message.",
                MAX_MSG_SIZE + 1
            )
        );
    }

    #[test]
    fn broken_streams_end_the_connection() {
        assert_eq!(
            run(caps::ZSTD, b"BEGIN2 'x' 1 2 #\n", 64),
            Err(Failure::Multiplexed)
        );
        assert_eq!(
            run(caps::GZIP, &frame(b"not gzip"), 64),
            Err(Failure::NoBytes)
        );
        assert_eq!(
            run(caps::BROTLI, &frame(&[0xff; 32]), 64),
            Err(Failure::NoBytes)
        );
        // A 2 MiB zstd bomb in a small frame is refused.
        let bomb = zstd::bulk::compress(&vec![0u8; 2 << 20], 1).unwrap();
        assert_eq!(run(caps::ZSTD, &frame(&bomb), 64), Err(Failure::NoBytes));
        assert!(Decompressor::for_capabilities(caps::V2).is_none());
    }

    #[test]
    fn failing_engines_log_cs_lines() {
        let line = |capabilities: u32, stream: &[u8]| {
            let (result, records) =
                netdata_agent_log::capture(|| run(capabilities, stream, 1 << 20));
            assert_eq!(result, Err(Failure::NoBytes));
            let messages: Vec<_> = records.into_iter().filter_map(|r| r.message).collect();
            assert_eq!(messages.len(), 1, "{messages:?}");
            messages[0].clone()
        };
        assert_eq!(
            line(caps::ZSTD, &frame(b"not zstd")),
            "STREAM_DECOMPRESS: ZSTD_decompressStream() return error: Unknown frame descriptor"
        );
        let bomb = zstd::bulk::compress(&vec![0u8; 2 << 20], 1).unwrap();
        assert!(line(caps::ZSTD, &frame(&bomb)).starts_with(&format!(
            "STREAM_DECOMPRESS: ZSTD_decompressStream() consumed only "
        )));
        // zlib: "incorrect header check" is Z_DATA_ERROR
        assert_eq!(
            line(caps::GZIP, &frame(b"not gzip")),
            "STREAM_DECOMPRESS: inflate() failed with error -3"
        );
        assert_eq!(
            line(caps::BROTLI, &frame(&[0xff; 32])),
            "STREAM_DECOMPRESS: Brotli decompression failed."
        );
        assert_eq!(
            line(caps::LZ4, &frame(&[0xf0, 1, 2])),
            "STREAM_DECOMPRESS: LZ4_decompress_safe_continue() returned negative value: -1 (compressed chunk is 3 bytes)"
        );
    }
}
