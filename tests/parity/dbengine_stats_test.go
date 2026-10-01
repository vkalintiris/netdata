// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"path/filepath"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// TestDbengineStats compares /api/v1/dbengine_stats (check `api.dbengine-stats`), the raw response with its clock
// headers masked, byte for byte: on the runR fixture's three tiers (`NETDATA_DBENGINE_FIXTURES`; skipped without
// it), on the size workload's tier, with the dbengine off (404), on an alloc localhost whose receivers use the
// dbengine, and before startup completes (503). The pulse charts are off: with them C's `currently_collected_metrics`
// and `disk_space` move with each side's own collection.
func TestDbengineStats(t *testing.T) {
	const path = "/api/v1/dbengine_stats"
	compare := func(t *testing.T, p *Pair, status string) {
		t.Helper()
		request := []byte("GET " + path + " HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
		var got [2][]byte
		for i, side := range p.Each() {
			b, err := rawExchange(side.Daemon.Addr, request, 10*time.Second)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			got[i] = maskRaw(b)
		}
		if !bytes.HasPrefix(got[0], []byte("HTTP/1.1 "+status+" ")) {
			t.Errorf("the oracle answers %q, expected %s", truncateBytes(got[0]), status)
		}
		if !bytes.Equal(got[0], got[1]) {
			t.Errorf("responses differ\n%s", firstDifference(got[0], got[1]))
		}
	}
	t.Run("runR", func(t *testing.T) {
		fx, id := runRParent(t)
		compare(t, StartPair(t, daemon.Options{StorageTiers: 3, TierRetentionMB: [3]int{25, 25, 25},
			SeedCache: filepath.Join(fx, "runR", "cache"), PulseOff: true}, id), "200")
	})
	t.Run("size", func(t *testing.T) {
		seed, _, _ := r2seed(t)
		compare(t, StartPair(t, daemon.Options{StorageTiers: 1, TierRetentionMB: [3]int{64}, SeedCache: seed,
			PulseOff: true}, parentIdentity), "200")
	})
	t.Run("disabled", func(t *testing.T) {
		compare(t, StartPair(t, daemon.Options{DBMode: "alloc", StreamMemoryMode: "ram", StorageTiers: 1,
			PulseOff: true}, parentIdentity), "404")
	})
	t.Run("receivers-only", func(t *testing.T) {
		compare(t, StartPair(t, daemon.Options{DBMode: "alloc", StreamMemoryMode: "dbengine", StorageTiers: 1,
			TierRetentionMB: [3]int{10}, PulseOff: true}, parentIdentity), "200")
	})
	// strm.conf.needs_dbengine (10e): each section asks for dbengine and C skips each: [stream] by name (enabled, not
	// sending: no destination), `enabled = no`, a db not exactly "dbengine". (No `enabled`: TestDbengineReceiverFallback.)
	t.Run("needs-none", func(t *testing.T) {
		compare(t, StartPair(t, daemon.Options{DBMode: "alloc", StreamMemoryMode: "ram", StorageTiers: 1, PulseOff: true,
			StreamTo: &daemon.StreamTo{APIKey: parentIdentity.StreamKey, Extra: "    db = dbengine\n"},
			StreamExtra: "\n[" + childHost.MachineGUID + "]\n    enabled = no\n    db = dbengine\n" +
				"\n[5a1e0000-0000-4000-8000-0000000000cc]\n    enabled = yes\n    db = DBENGINE\n"}, parentIdentity), "404")
	})
	t.Run("guid-section", func(t *testing.T) {
		compare(t, StartPair(t, daemon.Options{DBMode: "alloc", StreamMemoryMode: "ram", StorageTiers: 1,
			TierRetentionMB: [3]int{10}, PulseOff: true,
			StreamExtra: "\n[" + childHost.MachineGUID + "]\n    enabled = yes\n    db = dbengine\n"}, parentIdentity), "200")
	})
	t.Run("before-ready", func(t *testing.T) {
		compareBeforeReady(t, path)
	})
}
