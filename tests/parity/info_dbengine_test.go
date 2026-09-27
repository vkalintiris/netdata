// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/json"
	"fmt"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// infoDbengine is what /api/v1/info says about the database: the routed host's memory mode, the dbengine's disk
// quota and page cache size.
func infoDbengine(t *testing.T, d *daemon.Daemon, path string) string {
	t.Helper()
	b, err := rawExchange(d.Addr, []byte("GET "+path+" HTTP/1.1\r\nConnection: close\r\n\r\n"), 10*time.Second)
	if err != nil {
		t.Fatalf("%s: %v", d.Opts.Binary, err)
	}
	var doc map[string]any
	if err := json.Unmarshal(httpBody(b), &doc); err != nil {
		t.Fatalf("%s: %s: %v", d.Opts.Binary, path, err)
	}
	return fmt.Sprintf("memory-mode=%v multidb-disk-quota=%v page-cache-size=%v", doc["memory-mode"],
		doc["multidb-disk-quota"], doc["page-cache-size"])
}

// TestInfoDbengine compares /api/v1/info's dbengine members (check `api.v1-info-dbengine`): the memory mode of the
// routed host, `multidb-disk-quota` (tier 0's retention size as the dbengine's init reads it, 1024 when the engine
// does not start) and `page-cache-size` (after its clamp), each case also against the value C gives.
func TestInfoDbengine(t *testing.T) {
	cases := map[string]struct {
		opts  daemon.Options
		child bool
		want  string
	}{
		"default": {daemon.Options{StorageTiers: 1},
			false, "memory-mode=dbengine multidb-disk-quota=1024 page-cache-size=32"},
		"clamped": {daemon.Options{StorageTiers: 1, TierRetentionMB: [3]int{10},
			DBExtra: "    dbengine page cache size = 4MiB\n"},
			false, "memory-mode=dbengine multidb-disk-quota=25 page-cache-size=8"},
		"alloc-no-engine": {daemon.Options{DBMode: "alloc", StreamMemoryMode: "ram", StorageTiers: 1,
			TierRetentionMB: [3]int{10}},
			false, "memory-mode=alloc multidb-disk-quota=1024 page-cache-size=32"},
		"alloc-dbengine-children": {daemon.Options{DBMode: "alloc", StreamMemoryMode: "dbengine", StorageTiers: 1,
			TierRetentionMB: [3]int{10}},
			false, "memory-mode=alloc multidb-disk-quota=25 page-cache-size=32"},
		"no-quota": {daemon.Options{StorageTiers: 1, DBExtra: "    dbengine tier 0 retention size = 0\n"},
			false, "memory-mode=dbengine multidb-disk-quota=0 page-cache-size=32"},
		"ram-child": {daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1},
			true, "memory-mode=ram multidb-disk-quota=1024 page-cache-size=32"},
	}
	for name, c := range cases {
		t.Run(name, func(t *testing.T) {
			p := StartPair(t, c.opts, parentIdentity)
			path := "/api/v1/info"
			if c.child {
				for _, side := range p.Each() {
					conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, childHost, stream.CapsLive)
					if err != nil {
						t.Fatalf("%s: %v", side.Role, err)
					}
					t.Cleanup(func() { _ = conn.Close() })
					streamDataFixture(t, conn, time.Now().Unix()/60*60-120)
				}
				time.Sleep(2 * time.Second)
				path = "/host/" + childHost.Hostname + "/api/v1/info"
			}
			o, cand := infoDbengine(t, p.Oracle, path), infoDbengine(t, p.Candidate, path)
			if o != cand {
				t.Errorf("%s:\noracle:    %s\ncandidate: %s", path, o, cand)
			}
			if o != c.want {
				t.Errorf("%s: the oracle says %s, expected %s", path, o, c.want)
			}
		})
	}
}
