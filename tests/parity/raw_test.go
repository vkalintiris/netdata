// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"errors"
	"net"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// rawExchange sends request as-is and returns every byte the server sends back until it closes the connection
// or stays silent for timeout (a request C never completes shows up as "<timeout>").
func rawExchange(addr string, request []byte, timeout time.Duration) ([]byte, error) {
	conn, err := net.Dial("tcp", addr)
	if err != nil {
		return nil, err
	}
	defer conn.Close()
	if _, err := conn.Write(request); err != nil {
		return nil, err
	}
	var out bytes.Buffer
	buf := make([]byte, 64*1024)
	for {
		if err := conn.SetReadDeadline(time.Now().Add(timeout)); err != nil {
			return nil, err
		}
		n, err := conn.Read(buf)
		out.Write(buf[:n])
		if err != nil {
			var ne net.Error
			if errors.As(err, &ne) && ne.Timeout() {
				out.WriteString("<timeout>")
			}
			return out.Bytes(), nil
		}
	}
}

// rawMasks hide the header values that differ between any two responses (clock and random transaction ids).
var rawMasks = []*regexp.Regexp{
	regexp.MustCompile(`(?m)^(Date|Expires): [^\r]*`),
	regexp.MustCompile(`(?m)^X-Transaction-ID: [0-9a-f]*`),
}

func maskRaw(b []byte) []byte {
	for _, re := range rawMasks {
		b = re.ReplaceAllFunc(b, func(m []byte) []byte {
			name, _, _ := bytes.Cut(m, []byte(":"))
			return append(name, []byte(": <masked>")...)
		})
	}
	return b
}

// TestRawProtocol sends byte-exact requests that exercise the HTTP layer (method checks, OPTIONS, header
// parsing, limits) and compares the complete raw responses.
func TestRawProtocol(t *testing.T) {
	p := StartPair(t, daemon.Options{}, parentIdentity)
	cases := map[string][]byte{
		"unsupported-method":                   []byte("PATCH / HTTP/1.1\r\n\r\n"),
		"lowercase-method":                     []byte("get / HTTP/1.1\r\n\r\n"),
		"head-is-unsupported":                  []byte("HEAD /api/v1/info HTTP/1.1\r\n\r\n"),
		"options":                              []byte("OPTIONS /api/v1/info HTTP/1.1\r\nOrigin: http://x\r\n\r\n"),
		"options-mcp":                          []byte("OPTIONS /mcp HTTP/1.1\r\n\r\n"),
		"header-without-colon-never-completes": []byte("GET / HTTP/1.1\r\nFoo\r\n\r\n"),
		"uri-too-long": append(append([]byte("GET /"), bytes.Repeat([]byte("a"), 1<<20)...),
			[]byte(" HTTP/1.1\r\n\r\n")...),
	}
	for name, request := range cases {
		t.Run(name, func(t *testing.T) {
			var got [2][]byte
			for i, side := range p.Each() {
				b, err := rawExchange(side.Daemon.Addr, request, 2*time.Second)
				if err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				got[i] = maskRaw(b)
			}
			if !bytes.Equal(got[0], got[1]) {
				t.Errorf("responses differ\noracle:    %q\ncandidate: %q", truncateBytes(got[0]), truncateBytes(got[1]))
			}
		})
	}
}

func truncateBytes(b []byte) string {
	s := string(b)
	if len(s) > 600 {
		return s[:600] + "..."
	}
	return strings.ToValidUTF8(s, "?")
}
