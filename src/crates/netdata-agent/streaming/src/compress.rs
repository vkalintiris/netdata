//! The sending side of stream compression (`src/streaming/stream-compression/`, the cut of
//! `stream-sender-commit.c`): a commit is cut into pieces of at most 16255 bytes, and each piece becomes one message
//! of the connection's zstd, lz4, brotli or gzip stream, sent after its signature. zstd and gzip go through the C
//! libraries C links (byte for byte C's output with the same versions), lz4 and brotli through Rust encoders whose
//! output C's decoders read (D100.3, D101).

use brotli::enc::StandardAlloc;
use brotli::enc::encode::{BrotliEncoderOperation, BrotliEncoderParameter, BrotliEncoderStateStruct};
use flate2::{Compress, Compression, FlushCompress, Status};
use netdata_agent_log::{Priority, Source, nd_log};
use zstd::stream::raw::{Encoder as ZstdEncoder, InBuffer, Operation, OutBuffer};

use crate::compression::{Algorithm, LZ4_WINDOW, MAX_CHUNK, MAX_MSG_SIZE, SIGNATURE_MAX_PAYLOAD};
use crate::conf::CompressionLevels;

/// A compressor's records (`netdata_log_error()`).
fn failed(message: std::fmt::Arguments<'_>) {
    nd_log!(Source::Daemon, Priority::Err, "STREAM_COMPRESS: {message}");
}

/// The length of the next piece of `src` (`sender_commit()`'s cut): all of it up to 16255 bytes; with BINARY exactly
/// 16255; otherwise up to the last newline before that limit, or 16255 when the only newline is the first byte or
/// there is none (the line is split).
pub fn next_piece(src: &[u8], binary: bool) -> usize {
    if src.len() <= MAX_MSG_SIZE || binary {
        return src.len().min(MAX_MSG_SIZE);
    }
    match src[..MAX_MSG_SIZE].iter().rposition(|&b| b == b'\n') {
        Some(at) if at > 0 => at + 1,
        _ => MAX_MSG_SIZE,
    }
}

/// `simple_ring_buffer_make_room()` of an output buffer the compressors always write from its start: 16 KiB at
/// first, then doubled, or grown by the request when doubling is not enough. C prints the size in its records.
fn make_room(size: usize, wanted: usize) -> usize {
    if wanted <= size {
        return size;
    }
    let mut new_size = if size == 0 { MAX_CHUNK } else { size * 2 };
    if wanted > new_size {
        new_size = (new_size + wanted).max(wanted);
    }
    new_size
}

/// `ZSTD_CStreamOutSize()` of libzstd 1.5: a block's bound plus its header and a checksum.
const ZSTD_CSTREAM_OUT_SIZE: usize = (1 << 17) + ((1 << 17) >> 8) + 3 + 4;

/// One connection's compression state (`struct compressor_state`'s stream).
enum Engine {
    Zstd(Box<ZstdEncoder<'static>>),
    /// The last `LZ4_WINDOW` bytes compressed: what the next block may reference, as C's decoder keeps them.
    Lz4 { history: Vec<u8> },
    /// zlib's deflate with the gzip wrapper, `deflateInit2(level, Z_DEFLATED, 15 + 16, 8, Z_DEFAULT_STRATEGY)`.
    Gzip(Box<Compress>),
    Brotli(Box<BrotliEncoderStateStruct<StandardAlloc>>),
}

/// A connection's compressor (`struct compressor_state`).
pub struct Compressor {
    algorithm: Algorithm,
    level: i32,
    engine: Engine,
    output: Vec<u8>,
    /// The size of C's output ring, for its records.
    output_room: usize,
}

impl Compressor {
    /// `stream_compressor_init()` with the level of `levels` for `algorithm`, clamped as C clamps it (zstd 1 to its
    /// maximum, gzip 1 to 9, brotli 0 to 11); `None` when the engine cannot start (C's `initialized` stays false).
    pub fn new(algorithm: Algorithm, levels: &CompressionLevels) -> Option<Compressor> {
        let (level, engine) = match algorithm {
            Algorithm::Zstd => {
                let level = levels.zstd.clamp(1, zstd::zstd_safe::max_c_level());
                match ZstdEncoder::new(level) {
                    Ok(encoder) => (level, Engine::Zstd(Box::new(encoder))),
                    Err(e) => {
                        failed(format_args!("ZSTD_initCStream() returned error: {e}"));
                        return None;
                    }
                }
            }
            // LZ4_compress_fast_continue()'s acceleration; the Rust encoder has none
            Algorithm::Lz4 => (levels.lz4, Engine::Lz4 { history: Vec::new() }),
            Algorithm::Gzip => {
                let level = levels.gzip.clamp(1, 9);
                let compress = Compress::new_gzip(Compression::new(level as u32), 15);
                (level, Engine::Gzip(Box::new(compress)))
            }
            Algorithm::Brotli => {
                let level = levels.brotli.clamp(0, 11);
                let mut state = BrotliEncoderStateStruct::new(StandardAlloc::default());
                if !state.set_parameter(BrotliEncoderParameter::BROTLI_PARAM_QUALITY, level as u32) {
                    failed(format_args!("BrotliEncoderSetParameter() failed to set quality to {level}"));
                    return None;
                }
                (level, Engine::Brotli(Box::new(state)))
            }
        };
        Some(Compressor {
            algorithm,
            level,
            engine,
            output: Vec::new(),
            output_room: 0,
        })
    }

    pub fn algorithm(&self) -> Algorithm {
        self.algorithm
    }

    /// The level after C's clamp.
    pub fn level(&self) -> i32 {
        self.level
    }

    /// The first `len` bytes of the last message `compress()` produced.
    pub fn output(&self, len: usize) -> &[u8] {
        &self.output[..len]
    }

    /// `stream_compress()`: `piece`'s message (without its signature), or `None` after C's record: the engine failed,
    /// produced nothing, or more than a message may carry.
    pub fn compress(&mut self, piece: &[u8]) -> Option<&[u8]> {
        if piece.is_empty() {
            return None;
        }
        let size = match &mut self.engine {
            Engine::Zstd(encoder) => {
                self.output_room = make_room(self.output_room, zstd::zstd_safe::compress_bound(piece.len()).max(ZSTD_CSTREAM_OUT_SIZE));
                self.output.resize(self.output_room, 0);
                zstd(encoder, piece, &mut self.output)?
            }
            Engine::Lz4 { history } => {
                self.output_room = make_room(self.output_room, lz4_flex::block::get_maximum_output_size(piece.len()));
                self.output.resize(self.output_room, 0);
                let size = match lz4_flex::block::compress_into_with_dict(piece, &mut self.output, history) {
                    Ok(size) if size > 0 => size,
                    result => {
                        let size = result.map_or(-1, |s| s as i64);
                        failed(format_args!(
                            "LZ4_compress_fast_continue() returned {size} (source is {} bytes, output buffer can fit {} bytes)",
                            piece.len(),
                            self.output_room
                        ));
                        return None;
                    }
                };
                history.extend_from_slice(piece);
                let excess = history.len().saturating_sub(LZ4_WINDOW);
                history.drain(..excess);
                size
            }
            Engine::Gzip(compress) => {
                // deflateBound() of a piece fits the first 16 KiB of C's ring
                self.output_room = make_room(self.output_room, MAX_CHUNK);
                self.output.resize(self.output_room, 0);
                gzip(compress, piece, &mut self.output)?
            }
            Engine::Brotli(state) => {
                let bound = brotli::enc::encode::BrotliEncoderMaxCompressedSize(piece.len());
                self.output_room = make_room(self.output_room, bound.max(MAX_CHUNK));
                self.output.resize(self.output_room, 0);
                brotli_flush(state, piece, &mut self.output)?
            }
        };
        if size >= MAX_CHUNK || size > SIGNATURE_MAX_PAYLOAD {
            failed(format_args!(
                "compressed data is {size} bytes, exceeding max chunk size {MAX_CHUNK} or signature capacity \
                 {SIGNATURE_MAX_PAYLOAD}"
            ));
            return None;
        }
        Some(&self.output[..size])
    }
}

/// `stream_compress_zstd()`: `ZSTD_compressStream()`, and `ZSTD_flushStream()` when it produced nothing (libzstd
/// 1.5.7 buffers until a 128 KiB block fills, so the flush runs for every piece: D101.1).
fn zstd(encoder: &mut ZstdEncoder<'static>, piece: &[u8], output: &mut [u8]) -> Option<usize> {
    let room = output.len();
    let mut input = InBuffer::around(piece);
    let mut out = OutBuffer::around(output);
    if let Err(e) = encoder.run(&mut input, &mut out) {
        failed(format_args!("ZSTD_compressStream() return error: {e}"));
        return None;
    }
    if input.pos() < piece.len() {
        failed(format_args!(
            "ZSTD_compressStream() left unprocessed input (source payload {} bytes, consumed {} bytes)",
            piece.len(),
            input.pos()
        ));
        return None;
    }
    if out.pos() == 0 {
        if let Err(e) = encoder.flush(&mut out) {
            failed(format_args!("ZSTD_flushStream() return error: {e}"));
            return None;
        }
        if out.pos() == 0 {
            failed(format_args!(
                "ZSTD_compressStream() returned zero compressed bytes (source is {} bytes, output buffer can fit {room} \
                 bytes) ",
                piece.len()
            ));
            return None;
        }
    }
    Some(out.pos())
}

/// `stream_compress_gzip()`: `deflate(Z_SYNC_FLUSH)` into the output, with C's checks.
fn gzip(compress: &mut Compress, piece: &[u8], output: &mut [u8]) -> Option<usize> {
    let (before_in, before_out) = (compress.total_in(), compress.total_out());
    let status = compress.compress(piece, output, FlushCompress::Sync);
    let consumed = (compress.total_in() - before_in) as usize;
    let produced = (compress.total_out() - before_out) as usize;
    match status {
        Ok(Status::Ok | Status::StreamEnd) => {}
        // Z_BUF_ERROR, and zlib's stream errors
        Ok(Status::BufError) => {
            failed(format_args!("deflate() failed with error -5"));
            return None;
        }
        Err(_) => {
            failed(format_args!("deflate() failed with error -2"));
            return None;
        }
    }
    if consumed != piece.len() {
        failed(format_args!(
            "deflate() did not use all the input buffer, {} bytes out of {} remain",
            piece.len() - consumed,
            piece.len()
        ));
        return None;
    }
    if produced == output.len() {
        failed(format_args!(
            "deflate() needs a bigger output buffer than the one we provided (output buffer {} bytes, compressed \
             payload {} bytes)",
            output.len(),
            piece.len()
        ));
        return None;
    }
    if produced == 0 {
        failed(format_args!(
            "deflate() did not produce any output (output buffer {} bytes, compressed payload {} bytes)",
            output.len(),
            piece.len()
        ));
        return None;
    }
    Some(produced)
}

/// `stream_compress_brotli()`: one `BrotliEncoderCompressStream(BROTLI_OPERATION_FLUSH)`, with C's checks.
fn brotli_flush(
    state: &mut BrotliEncoderStateStruct<StandardAlloc>,
    piece: &[u8],
    output: &mut [u8],
) -> Option<usize> {
    let (mut available_in, mut in_offset) = (piece.len(), 0);
    let (mut available_out, mut out_offset) = (output.len(), 0);
    let mut total_out = None;
    let ok = state.compress_stream(
        BrotliEncoderOperation::BROTLI_OPERATION_FLUSH,
        &mut available_in,
        piece,
        &mut in_offset,
        &mut available_out,
        output,
        &mut out_offset,
        &mut total_out,
        &mut |_, _, _, _| (),
    );
    if !ok {
        failed(format_args!("Brotli compression failed."));
        return None;
    }
    if available_in != 0 {
        failed(format_args!(
            "BrotliEncoderCompressStream() did not use all the input buffer, {available_in} bytes out of {} remain",
            piece.len()
        ));
        return None;
    }
    if available_out == 0 {
        failed(format_args!(
            "BrotliEncoderCompressStream() needs a bigger output buffer than the one we provided (output buffer {} \
             bytes, compressed payload {} bytes)",
            output.len(),
            piece.len()
        ));
        return None;
    }
    let size = output.len() - available_out;
    if size == 0 {
        failed(format_args!(
            "BrotliEncoderCompressStream() did not produce any output from the input provided (input buffer {} bytes)",
            piece.len()
        ));
        return None;
    }
    Some(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps;
    use crate::compression::encode_signature;
    use crate::decompress::Decompressor;

    /// BINARY cuts at 16255 even where a newline would end the piece earlier; the edges of the newline search: a
    /// commit of 16255 bytes goes whole, a newline at 16255 is past the search, one at 1 counts.
    #[test]
    fn binary_pieces_split_mid_line() {
        let mut a = vec![b'x'; MAX_MSG_SIZE + 1];
        a[100] = b'\n';
        assert_eq!(next_piece(&a, false), 101);
        assert_eq!(next_piece(&a, true), MAX_MSG_SIZE, "mid-line");
        assert_eq!(next_piece(&a[..MAX_MSG_SIZE], false), MAX_MSG_SIZE, "whole");
        a[100] = b'x';
        a[MAX_MSG_SIZE] = b'\n';
        assert_eq!(next_piece(&a, false), MAX_MSG_SIZE, "a newline at 16255 is not searched");
        a[1] = b'\n';
        assert_eq!(next_piece(&a, false), 2);
        assert_eq!(next_piece(b"ab\ncd", true), 6 - 1);
    }

    /// `stream_compress()` refuses a message whose compressed size reaches 16384 bytes (the chunk, one past the
    /// signature's capacity), with C's record: on incompressible input each engine accepts up to 16383 and refuses the
    /// next size; zstd and lz4 grow a byte at a time, so they meet 16384 exactly.
    #[test]
    fn a_message_of_a_whole_chunk_is_refused() {
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let noise: Vec<u8> = (0..16_500)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        for (algorithm, exact) in [(Algorithm::Zstd, true), (Algorithm::Lz4, true), (Algorithm::Brotli, false)] {
            let mut accepted = 0;
            let mut refused = None;
            for n in 16_200..16_500 {
                let mut c = Compressor::new(algorithm, &CompressionLevels::of_profile(false)).unwrap();
                let (out, records) = netdata_agent_log::capture(|| c.compress(&noise[..n]).map(<[u8]>::len));
                match out {
                    Some(size) => accepted = accepted.max(size),
                    None => {
                        let messages: Vec<_> = records.into_iter().filter_map(|r| r.message).collect();
                        refused = Some(messages);
                        break;
                    }
                }
            }
            let refused = refused.unwrap_or_else(|| panic!("{algorithm:?}: never refused"));
            assert!(accepted < MAX_CHUNK, "{algorithm:?}: accepted {accepted}");
            assert_eq!(refused.len(), 1, "{algorithm:?}: {refused:?}");
            let size: usize = refused[0]
                .strip_prefix("STREAM_COMPRESS: compressed data is ")
                .and_then(|r| r.split(' ').next())
                .and_then(|n| n.parse().ok())
                .unwrap();
            assert_eq!(
                refused[0],
                format!(
                    "STREAM_COMPRESS: compressed data is {size} bytes, exceeding max chunk size 16384 or signature \
                     capacity 16383"
                )
            );
            if exact {
                assert_eq!((accepted, size), (MAX_CHUNK - 1, MAX_CHUNK), "{algorithm:?}");
            } else {
                assert!(size >= MAX_CHUNK, "{algorithm:?}: {size}");
            }
        }
    }

    #[test]
    fn pieces_cut_as_c() {
        let mut a = vec![b'x'; MAX_MSG_SIZE];
        assert_eq!(next_piece(&a, false), MAX_MSG_SIZE);
        a.push(b'y');
        assert_eq!(next_piece(&a, false), MAX_MSG_SIZE, "no newline: the line is split");
        a[100] = b'\n';
        assert_eq!(next_piece(&a, false), 101);
        a[100] = b'x';
        a[0] = b'\n';
        assert_eq!(next_piece(&a, false), MAX_MSG_SIZE, "a newline only at 0 does not count");
        a[MAX_MSG_SIZE - 1] = b'\n';
        assert_eq!(next_piece(&a, false), MAX_MSG_SIZE);
        assert_eq!(next_piece(&a, true), MAX_MSG_SIZE);
        assert_eq!(next_piece(b"short", true), 5);
        assert_eq!(next_piece(b"", false), 0);
    }

    /// C's ring sizes: 16 KiB first, then doubled or grown by the request.
    #[test]
    fn rooms_as_c() {
        assert_eq!(make_room(0, 100), MAX_CHUNK);
        assert_eq!(make_room(MAX_CHUNK, 100), MAX_CHUNK);
        assert_eq!(make_room(0, ZSTD_CSTREAM_OUT_SIZE), MAX_CHUNK + ZSTD_CSTREAM_OUT_SIZE);
        assert_eq!(make_room(MAX_CHUNK, MAX_CHUNK + 1), 2 * MAX_CHUNK);
    }

    /// Lines of varied sizes, like a child's.
    fn commits(n: usize) -> Vec<Vec<u8>> {
        (0..n)
            .map(|i| {
                let mut c = Vec::new();
                for j in 0..(i % 40 + 1) {
                    c.extend_from_slice(
                        format!("BEGIN2 'chart.{}' 1 {} #\nSET2 'd{j}' {i} {} A\nEND2\n", i % 13, 1_700_000_000 + i, i * j)
                            .as_bytes(),
                    );
                }
                c
            })
            .collect()
    }

    /// Every message decodes alone to its own piece with the receiver's decompressor, fed whole or in small steps.
    #[test]
    fn every_algorithm_decodes_message_by_message() {
        let levels = CompressionLevels::of_profile(false);
        for (algorithm, cap) in [
            (Algorithm::Zstd, caps::ZSTD),
            (Algorithm::Lz4, caps::LZ4),
            (Algorithm::Gzip, caps::GZIP),
            (Algorithm::Brotli, caps::BROTLI),
        ] {
            let mut c = Compressor::new(algorithm, &levels).unwrap();
            let mut d = Decompressor::for_capabilities(cap).unwrap();
            let mut plain = Vec::new();
            let mut total = 0usize;
            let mut compressed = 0usize;
            for (i, commit) in commits(1500).iter().enumerate() {
                let mut rest = &commit[..];
                while !rest.is_empty() {
                    let n = next_piece(rest, false);
                    let message = c.compress(&rest[..n]).unwrap().to_vec();
                    compressed += message.len();
                    let mut framed = encode_signature(message.len()).unwrap().to_vec();
                    framed.extend_from_slice(&message);
                    let step = [1, 7, framed.len()][i % 3];
                    for chunk in framed.chunks(step) {
                        d.feed(chunk);
                        while d.next_message(&mut plain).unwrap() {}
                    }
                    total += n;
                    // nothing held back: the parent has every piece as soon as its message arrived
                    assert_eq!(plain.len(), total, "{algorithm:?} message {i}");
                    rest = &rest[n..];
                }
            }
            assert_eq!(plain, commits(1500).concat(), "{algorithm:?}");
            assert!(compressed < total, "{algorithm:?} compresses");
        }
    }

    /// Large commits cut into pieces, lz4's history over several of C's decoder rings.
    #[test]
    fn large_commits_and_long_histories() {
        let levels = CompressionLevels::of_profile(true);
        let mut c = Compressor::new(Algorithm::Lz4, &levels).unwrap();
        let mut d = Decompressor::for_capabilities(caps::LZ4).unwrap();
        let big: Vec<u8> = commits(400).concat();
        assert!(big.len() > 3 * 114_688);
        let mut plain = Vec::new();
        let mut rest = &big[..];
        while !rest.is_empty() {
            let n = next_piece(rest, false);
            let message = c.compress(&rest[..n]).unwrap().to_vec();
            let mut framed = encode_signature(message.len()).unwrap().to_vec();
            framed.extend_from_slice(&message);
            d.feed(&framed);
            while d.next_message(&mut plain).unwrap() {}
            rest = &rest[n..];
        }
        assert_eq!(plain, big);
    }

    #[test]
    fn levels_clamp_as_c() {
        let wild = CompressionLevels { zstd: 99, lz4: 0, brotli: -3, gzip: 42 };
        assert_eq!(Compressor::new(Algorithm::Zstd, &wild).unwrap().level(), zstd::zstd_safe::max_c_level());
        assert_eq!(Compressor::new(Algorithm::Gzip, &wild).unwrap().level(), 9);
        assert_eq!(Compressor::new(Algorithm::Brotli, &wild).unwrap().level(), 0);
        assert_eq!(Compressor::new(Algorithm::Lz4, &wild).unwrap().level(), 0);
        assert!(Compressor::new(Algorithm::Zstd, &wild).unwrap().compress(b"").is_none());
    }
}
