package stream

import (
	"bytes"
	"testing"
)

// The signature as C's encoder writes it, the zstd RLE frame and the lz4 run decompressed by the agents' engines are
// checked by the parity check; here only their shapes.
func TestFrames(t *testing.T) {
	if got := Signature(16385); !bytes.Equal(got, []byte{0xfa, 0x81, 0x80, '\n'}) {
		t.Fatalf("Signature(16385) = %x", got)
	}
	if got := Signature(127); !bytes.Equal(got, []byte{0xfa, 0xff, 0x80, '\n'}) {
		t.Fatalf("Signature(127) = %x", got)
	}
	if got := len(ZstdRLE(16385, '\n')); got != 10 {
		t.Fatalf("ZstdRLE length %d", got)
	}
	// 16385 = 1 literal + 16372 matched + 12 literals: token, literal, offset, 64 x 255 + 33, token, 12 literals
	run := LZ4Run(16385, '\n')
	if len(run) != 4+65+1+12 || run[0] != 0x1f || run[4+64] != 33 || run[4+65] != 0xc0 {
		t.Fatalf("LZ4Run: %d bytes %x", len(run), run[:6])
	}
}
