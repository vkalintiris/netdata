package stream

// Compressed messages as a child's compressor writes them: a 4-byte signature carrying the message's length, then the
// bytes of one zstd or lz4 message (src/streaming/stream-compression/compression.h). The builders write frames by
// hand for repeated bytes, so the package needs no compression library.

// Signature is the header of a compressed message of n bytes (STREAM_COMPRESSION_SIGNATURE with 14 bits of length).
func Signature(n int) []byte {
	return []byte{'z' | 0x80, 0x80 | byte(n&0x7f), 0x80 | byte((n>>7)&0x7f), '\n'}
}

// ZstdRLE is a zstd frame that decompresses to n copies of b: one RLE block in a 32 KiB window (n at most 32768).
func ZstdRLE(n int, b byte) []byte {
	// the magic number, a frame header without a content size, the window descriptor, the last block's header (RLE)
	h := uint32(n)<<3 | 1<<1 | 1
	return []byte{0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x28, byte(h), byte(h >> 8), byte(h >> 16), b}
}

// LZ4Run is an lz4 block that decompresses to n copies of b (n at least 17): one literal, a match at offset 1 for all
// but the last 12 bytes, then 12 literals, as lz4 wants a block to end in literals.
func LZ4Run(n int, b byte) []byte {
	ml := n - 13 - 4
	token := byte(0x10)
	if ml >= 15 {
		token |= 0x0f
	} else {
		token |= byte(ml)
	}
	out := []byte{token, b, 1, 0}
	if ml >= 15 {
		rest := ml - 15
		for ; rest >= 255; rest -= 255 {
			out = append(out, 255)
		}
		out = append(out, byte(rest))
	}
	out = append(out, 0xc0)
	for range 12 {
		out = append(out, b)
	}
	return out
}

// WriteMessage writes one compressed message: its signature, then its bytes.
func (c *Conn) WriteMessage(payload []byte) error {
	return c.WriteRaw(append(Signature(len(payload)), payload...))
}

// ZstdRaw is a zstd frame holding `payload` as one raw block (at most 32 KiB).
func ZstdRaw(payload []byte) []byte {
	h := uint32(len(payload))<<3 | 1
	return append([]byte{0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x28, byte(h), byte(h >> 8), byte(h >> 16)}, payload...)
}

// LZ4Literals is an lz4 block holding `payload` as literals only.
func LZ4Literals(payload []byte) []byte {
	n := len(payload)
	if n < 15 {
		return append([]byte{byte(n) << 4}, payload...)
	}
	out := []byte{0xf0}
	rest := n - 15
	for ; rest >= 255; rest -= 255 {
		out = append(out, 255)
	}
	return append(append(out, byte(rest)), payload...)
}
