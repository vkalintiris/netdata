// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// The generator's window in the runR fixture (its README): three days of b6.c0..b6.c3 at 1 s, c3 without data in
// [start+100000, start+105000).
const (
	fixtureStart = 1789980541
	fixtureEnd   = 1790239740
)

// TestDbengineRead (check `dbengine.read`) starts both daemons, the C agent with its pulse charts off, on copies of
// the C-written runR fixture cache (`NETDATA_DBENGINE_FIXTURES`; skipped without it): three dbengine tiers of three
// days of an archived child's charts. Compared: the whole daemon log (the tiers' start with its file decisions, the
// registry's pre-population, the context loads, the exit), the archived child's contexts, and data queries on every
// tier, byte for byte.
func TestDbengineRead(t *testing.T) {
	fx := os.Getenv("NETDATA_DBENGINE_FIXTURES")
	if fx == "" {
		t.Skip("NETDATA_DBENGINE_FIXTURES unset")
	}
	guid, err := os.ReadFile(filepath.Join(fx, "runR", "lib", "registry", "netdata.public.unique.id"))
	if err != nil {
		t.Fatal(err)
	}
	id := daemon.Identity{Hostname: "b6parent", StreamKey: parentIdentity.StreamKey,
		MachineGUID: strings.TrimSpace(string(guid))}
	p := StartPair(t, daemon.Options{StorageTiers: 3, TierRetentionMB: [3]int{25, 25, 25},
		SeedCache: filepath.Join(fx, "runR", "cache"), PulseOff: true, LogsExtra: "    level = debug\n"}, id)

	t.Run("contexts", func(t *testing.T) {
		for _, path := range []string{"/host/b6child/api/v1/contexts", "/host/b6child/api/v1/contexts?options=full",
			"/host/b6child/api/v1/context?context=b6.ctx"} {
			compareGet(t, p, path)
		}
	})
	// Without tier=, C plans across tiers where S2 reads tier 0 (D62.4, until S4b): the automatic queries select
	// tier 0 explicitly, which C honours without filling gaps from other tiers.
	t.Run("data", func(t *testing.T) {
		host := "/host/b6child"
		mid := int64(fixtureStart + 100000)
		windows := map[string][2]int64{
			"start":   {fixtureStart, fixtureStart + 600},
			"gap":     {mid - 300, mid + 5300},
			"end":     {fixtureEnd - 900, fixtureEnd},
			"whole":   {fixtureStart, fixtureEnd},
			"outside": {fixtureStart - 3600, fixtureStart - 60},
		}
		for name, w := range windows {
			win := fmt.Sprintf("after=%d&before=%d", w[0], w[1])
			for _, q := range []string{
				"/api/v1/data?chart=b6.c3&" + win + "&points=20&tier=0&options=jsonwrap",
				"/api/v1/data?context=b6.ctx&" + win + "&points=12&tier=0&options=jsonwrap,debug",
				"/api/v1/data?chart=b6.c0&" + win + "&points=10&tier=1&options=jsonwrap,debug",
				"/api/v1/data?chart=b6.c1&" + win + "&points=10&tier=2&options=jsonwrap",
				"/api/v3/data?contexts=b6.ctx&" + win + "&points=8&tier=0&options=debug",
				"/api/v3/data?contexts=b6.ctx&" + win + "&points=8&tier=1",
				"/api/v3/data?contexts=b6.ctx&" + win + "&points=8&tier=2&group_by=dimension",
			} {
				t.Run(name+" "+q, func(t *testing.T) { compareGet(t, p, host+q) })
			}
		}
	})
	t.Run("records", func(t *testing.T) {
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		compareLogFiles(t, p, "daemon.log")
	})
}

// dbengineReadRules compare a JSON answer as the check does: the request timings and clock, a loaded context's version and
// post-processing times (clock values) masked; label sets unordered and the v3 summary's label keys masked, since C
// orders a label set by the labels' addresses (`rrdlabels.c` keys its JudyL by pointer), not by a contract.
var dbengineReadRules = Rules{
	Masks: []Mask{
		{Pattern: "**.timings", Reason: "request durations"},
		{Pattern: "agents.[].now", Reason: "the answer's clock"},
		{Pattern: "**.version", Reason: "context version: the clock of its post-processing"},
		{Pattern: "**.pp_last_queued", Reason: "clock"},
		{Pattern: "**.pp_last_dequeued", Reason: "clock"},
		{Pattern: "summary.labels", Reason: "label key order: C's label addresses"},
	},
	Unordered: []string{"**.labels"},
}

// compareGet compares both daemons' JSON answers to one request under dbengineReadRules.
func compareGet(t *testing.T, p *Pair, path string) {
	t.Helper()
	base, query, _ := strings.Cut(path, "?")
	params, err := url.ParseQuery(query)
	if err != nil {
		t.Fatal(err)
	}
	diffs, err := p.CompareJSON(base, params, dbengineReadRules)
	if err != nil && strings.Contains(err.Error(), "invalid JSON") {
		// a text answer (no metric matched): byte for byte
		var got [2][]byte
		for i, side := range p.Each() {
			b, err := rawExchange(side.Daemon.Addr, []byte("GET "+path+" HTTP/1.1\r\n\r\n"), 20*time.Second)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			got[i] = maskTimings(maskRaw(b))
		}
		if !bytes.Equal(got[0], got[1]) {
			t.Errorf("%s: responses differ\n%s", path, firstDifference(got[0], got[1]))
		}
		return
	}
	if err != nil {
		t.Fatal(err)
	}
	for _, d := range diffs {
		t.Errorf("%s: %s: oracle %s, candidate %s", path, d.Path, d.Oracle, d.Candidate)
	}
}
