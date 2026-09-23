// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"net/url"
	"os"
	"strconv"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/fixture"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

func TestMain(m *testing.M) {
	ScrubEnvironment()
	os.Exit(m.Run())
}

var (
	parentIdentity = daemon.Identity{
		Hostname:    "parity-parent",
		StreamKey:   "5a1e0000-0000-4000-8000-000000000001",
		MachineGUID: "5a1e0000-0000-4000-8000-0000000000aa",
	}
	childHost = stream.HostInfo{
		Hostname:    "parity-child",
		MachineGUID: "5a1e0000-0000-4000-8000-0000000000bb",
	}
)

// replicateInto boots nothing: it streams ch into every daemon of the pair as
// the same child and waits until both have the whole series.
func replicateInto(t *testing.T, p *Pair, ch fixture.Chart) {
	t.Helper()
	for _, side := range p.Each() {
		conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, childHost, stream.CapsReplication)
		if err != nil {
			t.Fatalf("%s: connect: %v", side.Role, err)
		}
		t.Cleanup(func() { _ = conn.Close() })
		if err := ch.Replicate(conn, 30*time.Second); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		if _, err := side.Daemon.WaitRetention(childHost.Hostname, ch.Context, ch.FirstT(), ch.LastT(), 15*time.Second); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
	}
}

// nullRules are what two runs of the SAME binary on the same inputs
// legitimately disagree on, found by this check (evidence in the status repo,
// harness/checks.md). Every differential check starts from them.
var nullRules = Rules{
	Masks: []Mask{
		// Request durations measured by the answering daemon.
		{Pattern: "**.timings.*", Reason: "request duration"},
		// A counter of context change events across all hosts, the parent's
		// own included, so its value depends on event timing. That it changes
		// when contexts change is a separate behavioural check.
		{Pattern: "**.versions.contexts_hard_hash", Reason: "context event counter"},
	},
	// The parent's own host labels are added by concurrent startup threads
	// (e.g. _aclk_available by the ACLK thread), so their order varies.
	Unordered: []string{"host_labels", "nodes.[].labels", "**.host_labels"},
	// The parent's own charts (and the context versions and collector list
	// derived from them) appear over its first seconds.
	Settle: 20 * time.Second,
}

// TestNullStreamedChart is the harness self-test: with PARITY_CANDIDATE set to
// the oracle binary, identical inputs must produce identical masked answers.
// A difference here is a missing mask or a harness bug, never an
// implementation difference.
func TestNullStreamedChart(t *testing.T) {
	p := StartPair(t, daemon.Options{}, parentIdentity)
	ch := fixture.FullPalette("parity.full_palette", "parity.full_palette", fixture.T0, 120)
	replicateInto(t, p, ch)

	after, before := strconv.FormatInt(ch.FirstT()-1, 10), strconv.FormatInt(ch.LastT(), 10)
	host := "/host/" + childHost.Hostname
	requests := map[string]struct {
		path   string
		params url.Values
	}{
		"v1-data-json": {host + "/api/v1/data", url.Values{
			"chart": {ch.ID}, "after": {after}, "before": {before}, "format": {"json"}}},
		"v1-data-json2-points": {host + "/api/v1/data", url.Values{
			"chart": {ch.ID}, "after": {after}, "before": {before}, "points": {"12"}, "format": {"json2"}}},
		"v3-data":     {host + "/api/v3/data", daemon.DataParams(ch.Context, ch.FirstT()-1, ch.LastT(), 120)},
		"v1-chart":    {host + "/api/v1/chart", url.Values{"chart": {ch.ID}}},
		"v1-charts":   {host + "/api/v1/charts", nil},
		"v3-contexts": {"/api/v3/contexts", url.Values{"scope_nodes": {childHost.Hostname}}},
		"v3-nodes":    {"/api/v3/nodes", nil},
		"v1-info":     {"/api/v1/info", nil},
	}
	for name, r := range requests {
		t.Run(name, func(t *testing.T) {
			diffs, err := p.CompareJSON(r.path, r.params, nullRules)
			if err != nil {
				t.Fatal(err)
			}
			for _, d := range diffs {
				t.Errorf("%s", d)
			}
		})
	}
}
