//! What both directions of stream compression share (`src/streaming/stream-compression/compression.h`): the message
//! sizes, the 4-byte signature before each compressed message, and the algorithms' fixed order.

use crate::caps;

/// `COMPRESSION_MAX_CHUNK`.
pub const MAX_CHUNK: usize = netdata_agent_pluginsd_proto::COMPRESSION_MAX_CHUNK;
/// `COMPRESSION_MAX_MSG_SIZE`: the largest piece of a commit, and the largest compressed message a receiver accepts.
pub const MAX_MSG_SIZE: usize = netdata_agent_pluginsd_proto::COMPRESSION_MAX_MSG_SIZE;
/// `STREAM_COMPRESSION_SIGNATURE_SIZE`.
pub const SIGNATURE_SIZE: usize = 4;
/// `STREAM_COMPRESSION_SIGNATURE_MAX_PAYLOAD_SIZE`: 14 bits of length.
pub const SIGNATURE_MAX_PAYLOAD: usize = (1 << 14) - 1;
/// How far back an lz4 block may reference.
pub const LZ4_WINDOW: usize = 65_536;

/// `STREAM_COMPRESSION_SIGNATURE`: `'z' | 0x80`, then two length bytes with their high bit set, then a newline.
const SIGNATURE: u32 = (b'z' as u32 | 0x80) | (0x80 << 8) | (0x80 << 16) | ((b'\n' as u32) << 24);
/// `STREAM_COMPRESSION_SIGNATURE_MASK`.
const SIGNATURE_MASK: u32 = 0xff | (0x80 << 8) | (0x80 << 16) | (0xff << 24);

/// `COMPRESSION_ALGORITHM`, in C's order of preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    Zstd,
    Lz4,
    Brotli,
    Gzip,
}

impl Algorithm {
    /// `stream_compression_initialize()` and `stream_decompression_initialize()`: the first of zstd, lz4, brotli and
    /// gzip the capabilities carry; none for an uncompressed stream.
    pub fn for_capabilities(capabilities: u32) -> Option<Algorithm> {
        [
            (caps::ZSTD, Algorithm::Zstd),
            (caps::LZ4, Algorithm::Lz4),
            (caps::BROTLI, Algorithm::Brotli),
            (caps::GZIP, Algorithm::Gzip),
        ]
        .into_iter()
        .find(|&(cap, _)| capabilities & cap != 0)
        .map(|(_, algorithm)| algorithm)
    }
}

/// `stream_compress_encode_signature()`: a host-endian `uint32` (little-endian here); `None` beyond 14 bits of length,
/// where C ends the agent.
pub fn encode_signature(len: usize) -> Option<[u8; SIGNATURE_SIZE]> {
    if len > SIGNATURE_MAX_PAYLOAD {
        return None;
    }
    let len = len as u32;
    let length_bytes = ((len & 0x7f) | 0x80 | (((len & (0x7f << 7)) << 1) | 0x8000)) << 8;
    Some((length_bytes | (b'z' as u32 | 0x80) | ((b'\n' as u32) << 24)).to_le_bytes())
}

/// `stream_decompress_decode_signature()`: the compressed length, or `None` when the bytes are not a signature
/// (uncompressed data inside a compressed stream).
pub fn decode_signature(bytes: [u8; SIGNATURE_SIZE]) -> Option<usize> {
    let sign = u32::from_le_bytes(bytes);
    if sign & SIGNATURE_MASK != SIGNATURE {
        return None;
    }
    Some((((sign >> 8) & 0x7f) | ((sign >> 9) & (0x7f << 7))) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every length round-trips; the bytes are C's (`unittest_stream_compression_signature()`).
    #[test]
    fn signatures_as_c() {
        for len in 0..=SIGNATURE_MAX_PAYLOAD {
            assert_eq!(decode_signature(encode_signature(len).unwrap()), Some(len), "{len}");
        }
        assert_eq!(encode_signature(SIGNATURE_MAX_PAYLOAD + 1), None);
        assert_eq!(encode_signature(1), Some([0xfa, 0x81, 0x80, 0x0a]));
        assert_eq!(encode_signature(16255), Some([0xfa, 0xff, 0xfe, 0x0a]));
        assert_eq!(decode_signature(*b"CHAR"), None);
    }

    #[test]
    fn algorithms_in_c_order() {
        let all = caps::ZSTD | caps::LZ4 | caps::BROTLI | caps::GZIP;
        assert_eq!(Algorithm::for_capabilities(all), Some(Algorithm::Zstd));
        assert_eq!(Algorithm::for_capabilities(all & !caps::ZSTD), Some(Algorithm::Lz4));
        assert_eq!(Algorithm::for_capabilities(caps::GZIP | caps::BROTLI), Some(Algorithm::Brotli));
        assert_eq!(Algorithm::for_capabilities(caps::GZIP), Some(Algorithm::Gzip));
        assert_eq!(Algorithm::for_capabilities(caps::VCAPS), None);
    }
}
