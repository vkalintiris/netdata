//! The receiving side of stream compression (`src/streaming/stream-compression/`): each compressed message is a
//! 4-byte signature carrying its length, followed by that many bytes of one continuous zstd, lz4, brotli or gzip
//! stream per connection. Decompressed output feeds the line reader. A message that fails any of C's checks ends
//! the connection.

use brotli_decompressor::{BrotliDecompressStream, BrotliResult, BrotliState, StandardAlloc};
use flate2::{Crc, Decompress, FlushDecompress, Status};
use zstd::stream::raw::{Decoder as ZstdDecoder, InBuffer, Operation, OutBuffer};

use crate::caps;

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

/// The gzip member header zlib's `inflate()` parses before the deflate data (RFC 1952).
#[derive(Debug, Default)]
struct GzipHeader {
    /// Bytes of the fixed part seen so far.
    fixed: Vec<u8>,
    /// What remains to skip after the fixed part: extra field, name, comment, header CRC.
    stage: GzipStage,
    extra_left: Option<usize>,
    extra_len: Vec<u8>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum GzipStage {
    #[default]
    Fixed,
    Extra,
    Name,
    Comment,
    HeaderCrc(u8),
    Done,
}

const FHCRC: u8 = 1 << 1;
const FEXTRA: u8 = 1 << 2;
const FNAME: u8 = 1 << 3;
const FCOMMENT: u8 = 1 << 4;

impl GzipHeader {
    /// Consumes header bytes from `input`; returns how many it took, or `Err` for a stream that is not gzip.
    fn consume(&mut self, input: &[u8]) -> Result<usize, ()> {
        let mut i = 0;
        while i < input.len() && self.stage != GzipStage::Done {
            let b = input[i];
            let flags = self.fixed.get(3).copied().unwrap_or(0);
            match self.stage {
                GzipStage::Fixed => {
                    self.fixed.push(b);
                    i += 1;
                    if self.fixed.len() == 1 && b != 0x1f
                        || self.fixed.len() == 2 && b != 0x8b
                        || self.fixed.len() == 3 && b != 8
                        || self.fixed.len() == 4 && b & 0xe0 != 0
                    {
                        return Err(());
                    }
                    if self.fixed.len() == 10 {
                        self.stage = self.after(GzipStage::Fixed, flags_of(&self.fixed));
                    }
                }
                GzipStage::Extra => {
                    match self.extra_left {
                        None => {
                            self.extra_len.push(b);
                            if self.extra_len.len() == 2 {
                                self.extra_left = Some(usize::from(u16::from_le_bytes([
                                    self.extra_len[0],
                                    self.extra_len[1],
                                ])));
                            }
                        }
                        Some(left) => self.extra_left = Some(left - 1),
                    }
                    i += 1;
                    if self.extra_left == Some(0) {
                        self.stage = self.after(GzipStage::Extra, flags);
                    }
                }
                GzipStage::Name | GzipStage::Comment => {
                    i += 1;
                    if b == 0 {
                        self.stage = self.after(self.stage, flags);
                    }
                }
                GzipStage::HeaderCrc(seen) => {
                    i += 1;
                    self.stage = if seen == 1 {
                        GzipStage::Done
                    } else {
                        GzipStage::HeaderCrc(seen + 1)
                    };
                }
                GzipStage::Done => {}
            }
        }
        Ok(i)
    }

    /// The stage after `stage`, given the header flags.
    fn after(&self, stage: GzipStage, flags: u8) -> GzipStage {
        let order = [
            (GzipStage::Extra, FEXTRA),
            (GzipStage::Name, FNAME),
            (GzipStage::Comment, FCOMMENT),
            (GzipStage::HeaderCrc(0), FHCRC),
        ];
        let from = match stage {
            GzipStage::Fixed => 0,
            GzipStage::Extra => 1,
            GzipStage::Name => 2,
            GzipStage::Comment => 3,
            _ => 4,
        };
        order[from..]
            .iter()
            .find(|(_, flag)| flags & flag != 0)
            .map_or(GzipStage::Done, |&(next, _)| next)
    }
}

fn flags_of(fixed: &[u8]) -> u8 {
    fixed[3]
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
    Gzip {
        header: GzipHeader,
        inflate: Box<Decompress>,
        crc: Crc,
        /// The member ended: the next 8 bytes are its trailer.
        trailer: Option<Vec<u8>>,
    },
    Brotli(Box<BrotliState<StandardAlloc, StandardAlloc, StandardAlloc>>),
}

/// Why the connection ends; the text is C's log message.
pub type Failure = &'static str;

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
                Engine::Gzip {
                    header: GzipHeader::default(),
                    inflate: Box::new(Decompress::new(false)),
                    crc: Crc::new(),
                    trailer: None,
                },
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
                break Err("multiplexed uncompressed data in compressed stream!");
            };
            if size > MAX_MSG_SIZE {
                break Err(
                    "received a compressed message bigger than the max compressed message size supported",
                );
            }
            let body = start + SIGNATURE_SIZE;
            if body + size > self.pending.len() {
                break Ok(());
            }
            let message = self.pending[body..body + size].to_vec();
            match self.decompress(&message) {
                Ok(0) | Err(_) => break Err("no bytes to decompress."),
                Ok(n) => out.extend_from_slice(&self.output[..n]),
            }
            start = body + size;
        };
        self.pending.drain(..start);
        result
    }

    /// `stream_decompress()`: one message into the output buffer; the decompressed length (0 is a failure too).
    fn decompress(&mut self, data: &[u8]) -> Result<usize, ()> {
        let output = &mut self.output;
        match &mut self.engine {
            Engine::Zstd(decoder) => {
                let mut input = InBuffer::around(data);
                let mut out = OutBuffer::around(&mut output[..]);
                decoder.run(&mut input, &mut out).map_err(|_| ())?;
                // A frame decompressing to more than the buffer is refused.
                if input.pos < data.len() {
                    return Err(());
                }
                Ok(out.pos())
            }
            Engine::Lz4 { history, write_pos } => {
                if *write_pos + MAX_CHUNK > LZ4_RING {
                    *write_pos = 0;
                }
                let room = LZ4_RING - *write_pos;
                let n =
                    lz4_flex::block::decompress_into_with_dict(data, &mut output[..room], history)
                        .map_err(|_| ())?;
                *write_pos += n;
                history.extend_from_slice(&output[..n]);
                if history.len() > LZ4_WINDOW {
                    history.drain(..history.len() - LZ4_WINDOW);
                }
                Ok(n)
            }
            Engine::Gzip {
                header,
                inflate,
                crc,
                trailer,
            } => {
                let mut data = data;
                let taken = header.consume(data)?;
                data = &data[taken..];
                let mut produced = 0;
                if header.stage == GzipStage::Done && trailer.is_none() {
                    let (before_in, before_out) = (inflate.total_in(), inflate.total_out());
                    let status = inflate
                        .decompress(data, &mut output[..], FlushDecompress::Sync)
                        .map_err(|_| ())?;
                    let consumed = (inflate.total_in() - before_in) as usize;
                    produced = (inflate.total_out() - before_out) as usize;
                    crc.update(&output[..produced]);
                    data = &data[consumed..];
                    match status {
                        Status::StreamEnd => *trailer = Some(Vec::new()),
                        Status::Ok => {}
                        // zlib's Z_BUF_ERROR: no progress was possible.
                        Status::BufError => return Err(()),
                    }
                }
                if let Some(t) = trailer {
                    let take = data.len().min(8 - t.len());
                    t.extend_from_slice(&data[..take]);
                    data = &data[take..];
                    if t.len() == 8 {
                        let want_crc = u32::from_le_bytes([t[0], t[1], t[2], t[3]]);
                        let want_len = u32::from_le_bytes([t[4], t[5], t[6], t[7]]);
                        if want_crc != crc.sum() || want_len != crc.amount() {
                            return Err(());
                        }
                    }
                }
                // Every compressed byte must be used, and the output must not fill the buffer.
                if !data.is_empty() || produced == output.len() {
                    return Err(());
                }
                Ok(produced)
            }
            Engine::Brotli(state) => {
                let (mut available_in, mut input_offset) = (data.len(), 0);
                let (mut available_out, mut output_offset, mut total_out) = (output.len(), 0, 0);
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
                if matches!(result, BrotliResult::ResultFailure)
                    || available_in != 0
                    || available_out == 0
                {
                    return Err(());
                }
                Ok(output.len() - available_out)
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
    fn broken_streams_end_the_connection() {
        assert_eq!(
            run(caps::ZSTD, b"BEGIN2 'x' 1 2 #\n", 64),
            Err("multiplexed uncompressed data in compressed stream!")
        );
        assert_eq!(
            run(caps::GZIP, &frame(b"not gzip"), 64),
            Err("no bytes to decompress.")
        );
        assert_eq!(
            run(caps::BROTLI, &frame(&[0xff; 32]), 64),
            Err("no bytes to decompress.")
        );
        // A 2 MiB zstd bomb in a small frame is refused.
        let bomb = zstd::bulk::compress(&vec![0u8; 2 << 20], 1).unwrap();
        assert_eq!(
            run(caps::ZSTD, &frame(&bomb), 64),
            Err("no bytes to decompress.")
        );
        assert!(Decompressor::for_capabilities(caps::V2).is_none());
    }
}
