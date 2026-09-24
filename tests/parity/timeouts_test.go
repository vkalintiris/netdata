// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"net"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// closedWithin reports how long the daemon took to close a connection that sent request (possibly nothing), after
// reading its answer; zero when it stayed open for max.
func closedWithin(t *testing.T, addr string, request []byte, max time.Duration) time.Duration {
	t.Helper()
	conn, err := net.Dial("tcp", addr)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	start := time.Now()
	if len(request) > 0 {
		if _, err := conn.Write(request); err != nil {
			t.Fatal(err)
		}
	}
	buf := make([]byte, 64*1024)
	_ = conn.SetReadDeadline(start.Add(max))
	for {
		if _, err := conn.Read(buf); err != nil {
			if ne, ok := err.(net.Error); ok && ne.Timeout() {
				return 0
			}
			return time.Since(start)
		}
	}
}

// TestWebTimeouts compares the first-request and idle timeouts (both close the connection at the next cleanup pass,
// every idle/3+1 seconds) and the streaming rate limit.
func TestWebTimeouts(t *testing.T) {
	extra := "    timeout for first request = 2\n" +
		"    disconnect idle clients after = 3\n" +
		"    accept a streaming request every = 60\n"
	p := StartPair(t, daemon.Options{WebExtra: extra, StreamMemoryMode: "ram", StorageTiers: 1}, parentIdentity)
	keepalive := []byte("GET /api/v1/info HTTP/1.1\r\nConnection: keep-alive\r\n\r\n")
	for _, side := range p.Each() {
		t.Run(string(side.Role)+"/first-request", func(t *testing.T) {
			// C's first cleanup pass can come several passes late; seen up to ~9s.
			d := closedWithin(t, side.Daemon.Addr, nil, 16*time.Second)
			if d < 2*time.Second || d > 15*time.Second {
				t.Errorf("closed after %v, want 2s..15s (0 = never)", d)
			}
		})
		t.Run(string(side.Role)+"/idle", func(t *testing.T) {
			d := closedWithin(t, side.Daemon.Addr, keepalive, 14*time.Second)
			if d < 3*time.Second || d > 10*time.Second {
				t.Errorf("closed after %v, want 3s..10s (0 = never)", d)
			}
		})
	}
	// The first STREAM request passes the rate limit (and is then taken over); the second, within the period, is
	// refused with the busy message.
	stream := func(d *daemon.Daemon, guid string) []byte {
		req := fmt.Sprintf("STREAM key=%s&hostname=rate-%s&registry_hostname=rate-%s&machine_guid=%s"+
			"&update_every=1&os=linux&timezone=Etc/UTC&abbrev_timezone=UTC&utc_offset=0&hops=1&ver=1"+
			"&NETDATA_PROTOCOL_VERSION=1.1 HTTP/1.1\r\nUser-Agent: parity/1.0\r\n\r\n",
			d.StreamKey, guid[len(guid)-4:], guid[len(guid)-4:], guid)
		b, _ := rawExchange(d.Addr, []byte(req), 2*time.Second)
		return b
	}
	var got [2][]byte
	for i, side := range p.Each() {
		_ = stream(side.Daemon, "00000000-0000-4000-8000-00000000a001")
		got[i] = maskRaw(stream(side.Daemon, "00000000-0000-4000-8000-00000000a002"))
	}
	if string(got[0]) != string(got[1]) {
		t.Errorf("rate-limited STREAM differs\noracle:    %q\ncandidate: %q", got[0], got[1])
	}
}
