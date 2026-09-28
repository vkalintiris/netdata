// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"fmt"
	"regexp"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// chartEntry is what the waits read of a chart in /api/v1/charts.
type chartEntry struct {
	LastEntry int64 `json:"last_entry"`
}

// hostCharts fetches a host's /api/v1/charts ("" for localhost) on a connection of its own, tagged so the log checks
// leave it out (probeRe): the chart ids in the answer's order and each chart's last entry. Anything but a 200 with a
// `charts` member is an error.
func hostCharts(d *daemon.Daemon, host string) ([]string, map[string]chartEntry, error) {
	path := "/api/v1/charts?harness=wait"
	if host != "" {
		path = "/host/" + host + path
	}
	b, err := rawExchange(d.Addr, []byte("GET "+path+" HTTP/1.1\r\nConnection: close\r\n\r\n"), 10*time.Second)
	if err != nil {
		return nil, nil, err
	}
	if !bytes.HasPrefix(b, []byte("HTTP/1.1 200 ")) {
		return nil, nil, fmt.Errorf("%s: %q", path, truncateBytes(b))
	}
	var doc struct {
		Charts map[string]chartEntry `json:"charts"`
	}
	if err := json.Unmarshal(httpBody(b), &doc); err != nil {
		return nil, nil, fmt.Errorf("%s: %v", path, err)
	}
	if doc.Charts == nil {
		return nil, nil, fmt.Errorf("%s: no charts member", path)
	}
	v, err := ParseJSON(httpBody(b))
	if err != nil {
		return nil, nil, fmt.Errorf("%s: %v", path, err)
	}
	for _, m := range v.Members {
		if m.Key == "charts" {
			return memberKeys(m.Value), doc.Charts, nil
		}
	}
	return nil, nil, fmt.Errorf("%s: no charts member", path)
}

// memoryBytesRe finds /api/v1/charts' memory figure: each implementation reports its own structures.
var memoryBytesRe = regexp.MustCompile(`"rrd_memory_bytes":[0-9]+`)

// TestChartsAPI streams the data fixture into both daemons and compares /api/v1/charts and /api/v1/chart for the
// child (localhost's pulse charts are compared by `pulse.localhost-charts`).
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
		t.Run(name, func(t *testing.T) { compareChartsRaw(t, p, path) })
	}
}

// compareChartsRaw compares both daemons' raw answers to one request, byte for byte, with the timings and each
// side's memory figure masked.
func compareChartsRaw(t *testing.T, p *Pair, path string) {
	t.Helper()
	var got [2][]byte
	for i, side := range p.Each() {
		b, err := rawExchange(side.Daemon.Addr, []byte("GET "+path+" HTTP/1.1\r\n\r\n"), 20*time.Second)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		got[i] = memoryBytesRe.ReplaceAll(maskTimings(maskRaw(b)), []byte(`"rrd_memory_bytes":"<masked>"`))
	}
	if !bytes.Equal(got[0], got[1]) && !labelOrderOnly(got[0], got[1]) {
		t.Errorf("%s: responses differ\n%s", path, firstDifference(got[0], got[1]))
	}
}
