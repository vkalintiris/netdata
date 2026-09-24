// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"net"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// streamRequest is a STREAM request line as children send it.
func streamRequest(query string) []byte {
	return []byte("STREAM " + query + " HTTP/1.1\r\nUser-Agent: parity-child/1.0\r\nAccept: */*\r\n\r\n")
}

// TestStreamHandshake compares the receiver's answers to STREAM requests: accepted prompts per protocol version,
// every refusal decided before the host exists, the parent's own GUID, and a second connection for a GUID that is
// still connected. Compression is not requested: the Rust receiver offers none until its decompressors land (D14).
func TestStreamHandshake(t *testing.T) {
	p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1}, parentIdentity)
	key := parentIdentity.StreamKey
	guid := func(n int) string { return fmt.Sprintf("5a1e0000-0000-4000-8000-%012d", 100+n) }
	query := func(n int, extra string) string {
		return fmt.Sprintf("key=%s&hostname=hs-%d&machine_guid=%s&update_every=1%s", key, n, guid(n), extra)
	}
	steps := []struct {
		name    string
		request []byte
	}{
		{"vcaps", streamRequest(query(1, "&ver=17088&NETDATA_PROTOCOL_VERSION=1.1"))},
		{"v1", streamRequest(query(2, "&ver=1"))},
		{"v2", streamRequest(query(3, "&ver=2"))},
		{"vn4", streamRequest(query(4, "&ver=4"))},
		{"protocol-version-before-ver", streamRequest(query(5, "&NETDATA_PROTOCOL_VERSION=1.1&ver=17088"))},
		{"replication-caps", streamRequest(query(6, "&ver=21184"))},
		{"no-key", streamRequest(fmt.Sprintf("hostname=x&machine_guid=%s&ver=17088", guid(7)))},
		{"no-hostname", streamRequest(fmt.Sprintf("key=%s&machine_guid=%s&ver=17088", key, guid(8)))},
		{"no-guid", streamRequest(fmt.Sprintf("key=%s&hostname=x&ver=17088", key))},
		{"key-not-enabled", streamRequest(fmt.Sprintf("key=%s&hostname=x&machine_guid=%s&ver=17088", guid(9), guid(10)))},
		{"invalid-hops", streamRequest(query(11, "&ver=17088&hops=0"))},
		{"own-guid", streamRequest(fmt.Sprintf("key=%s&hostname=x&machine_guid=%s&ver=17088", key, parentIdentity.MachineGUID))},
	}
	for _, step := range steps {
		t.Run(step.name, func(t *testing.T) {
			var got [2][]byte
			for i, side := range p.Each() {
				b, err := rawExchange(side.Daemon.Addr, step.request, time.Second)
				if err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				got[i] = b
			}
			if !bytes.Equal(got[0], got[1]) {
				t.Errorf("responses differ\noracle:    %q\ncandidate: %q", got[0], got[1])
			}
		})
	}

	// A second connection while the first one is still attached (and quiet for less than 30 s) is refused.
	t.Run("already-streaming", func(t *testing.T) {
		var got [2][]byte
		for i, side := range p.Each() {
			held, err := net.Dial("tcp", side.Daemon.Addr)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			defer held.Close()
			if _, err := held.Write(streamRequest(query(12, "&ver=17088"))); err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			prompt := make([]byte, 128)
			_ = held.SetReadDeadline(time.Now().Add(2 * time.Second))
			n, _ := held.Read(prompt)
			second, err := rawExchange(side.Daemon.Addr, streamRequest(query(12, "&ver=17088")), time.Second)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			got[i] = append(append(prompt[:n:n], '|'), second...)
		}
		if !bytes.Equal(got[0], got[1]) {
			t.Errorf("responses differ\noracle:    %q\ncandidate: %q", got[0], got[1])
		}
	})
}
