// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"regexp"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// memoryBytesRe finds /api/v1/charts' memory figure: each implementation reports its own structures.
var memoryBytesRe = regexp.MustCompile(`"rrd_memory_bytes":[0-9]+`)

// TestChartsAPI streams the data fixture into both daemons and compares /api/v1/charts and /api/v1/chart for the
// child (the parents' own charts differ by design).
func TestChartsAPI(t *testing.T) {
	p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1}, parentIdentity)
	base := time.Now().Unix()/60*60 - 120
	for _, side := range p.Each() {
		conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, childHost, stream.CapsLive)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		t.Cleanup(func() { _ = conn.Close() })
		streamDataFixture(t, conn, base)
	}
	time.Sleep(2500 * time.Millisecond)
	host := "/host/" + childHost.Hostname
	cases := map[string]string{
		"charts":        host + "/api/v1/charts",
		"chart":         host + "/api/v1/chart?chart=q.a",
		"chart-by-name": host + "/api/v1/chart?chart=q_a_name",
		"chart-two":     host + "/api/v1/chart?chart=q.two&chart=q.two",
		"chart-missing": host + "/api/v1/chart?chart=no%3Cpe",
		"chart-none":    host + "/api/v1/chart",
		"chart-empty":   host + "/api/v1/chart?chart=",
	}
	for name, path := range cases {
		t.Run(name, func(t *testing.T) {
			var got [2][]byte
			for i, side := range p.Each() {
				b, err := rawExchange(side.Daemon.Addr, []byte("GET "+path+" HTTP/1.1\r\n\r\n"), 2*time.Second)
				if err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				got[i] = memoryBytesRe.ReplaceAll(maskTimings(maskRaw(b)), []byte(`"rrd_memory_bytes":"<masked>"`))
			}
			if !bytes.Equal(got[0], got[1]) {
				t.Errorf("responses differ\n%s", firstDifference(got[0], got[1]))
			}
		})
	}
}
