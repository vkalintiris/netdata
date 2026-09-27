// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"math"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// lazyPulse are the pulse charts created once their data comes, in whichever cycle that is (D80.6): the traffic,
// the points generated and each child's charts.
var lazyPulse = regexp.MustCompile(`^netdata\.(network_api|network_streaming|db_points_results|streaming\.in\.)`)

// pulseChartsRules compare localhost's /api/v1/charts: every chart's definition exactly, the charts in any order
// (the lazy ones are created when their data comes; the order of the others is checked apart), each side's clock
// and memory figures masked.
var pulseChartsRules = Rules{
	Masks: []Mask{
		{Pattern: "charts.*.first_entry", Reason: "each daemon's own start"},
		{Pattern: "charts.*.last_entry", Reason: "the clock"},
		{Pattern: "charts.*.duration", Reason: "each daemon's own start"},
		{Pattern: "rrd_memory_bytes", Reason: "each implementation's own structures (D24)"},
	},
	// C keeps a chart's labels in pointer order (D22.2)
	Unordered: []string{"charts", "charts.*.chart_labels"},
}

// localData is /api/v1/data of a localhost chart as CSV, one point per second of [after, before] (one for the whole
// window when grouped by sum or max), each value rounded (the stored numbers' precision). chart may carry more
// parameters.
func localData(t *testing.T, d *daemon.Daemon, chart string, after, before int64, group string) string {
	t.Helper()
	points, options := before-after+1, ""
	if group == "sum" || group == "max" {
		// one point for the whole window, not aligned to its length (which could move it past the last point)
		points, options = 1, "&options=unaligned"
	}
	req := fmt.Sprintf("GET /api/v1/data?chart=%s&after=%d&before=%d&points=%d&group=%s%s&format=csv&harness=wait "+
		"HTTP/1.1\r\nConnection: close\r\n\r\n", chart, after, before, points, group, options)
	b, err := rawExchange(d.Addr, []byte(req), 10*time.Second)
	if err != nil {
		t.Fatalf("%s: %v", d.Opts.Binary, err)
	}
	lines := strings.Split(strings.TrimSpace(string(httpBody(b))), "\n")
	for i := 1; i < len(lines); i++ {
		cells := strings.Split(strings.TrimSpace(lines[i]), ",")
		for j := 1; j < len(cells); j++ {
			if v, err := strconv.ParseFloat(cells[j], 64); err == nil {
				cells[j] = strconv.FormatFloat(math.Round(v), 'f', 0, 64)
			}
		}
		lines[i] = strings.Join(cells, ",")
	}
	return strings.Join(lines, "\n")
}

// compareLocalData compares a localhost chart's values over the same seconds on both sides.
func compareLocalData(t *testing.T, p *Pair, chart string, after, before int64, group string) {
	t.Helper()
	var got [2]string
	for i, side := range p.Each() {
		got[i] = localData(t, side.Daemon, chart, after, before, group)
	}
	if got[0] != got[1] {
		t.Errorf("%s [%d, %d] %s:\noracle:\n%s\ncandidate:\n%s", chart, after, before, group, got[0], got[1])
	}
}

// localCharts are localhost's chart ids in /api/v1/charts' order.
func localCharts(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	b, err := rawExchange(d.Addr, []byte("GET /api/v1/charts?harness=wait HTTP/1.1\r\nConnection: close\r\n\r\n"),
		10*time.Second)
	if err != nil {
		t.Fatalf("%s: %v", d.Opts.Binary, err)
	}
	v, err := ParseJSON(httpBody(b))
	if err != nil {
		t.Fatalf("%s: %v", d.Opts.Binary, err)
	}
	for _, m := range v.Members {
		if m.Key == "charts" {
			return memberKeys(m.Value)
		}
	}
	return nil
}

// firstCycle drops the lazy charts from ids.
func firstCycle(ids []string) []string {
	return slices.DeleteFunc(slices.Clone(ids), func(id string) bool { return lazyPulse.MatchString(id) })
}

// TestPulseLocalhostCharts compares localhost's pulse charts (check `pulse.localhost-charts`): C's chart definitions
// in C's first-cycle order, the lazy charts as a set, in an alloc and a dbengine localhost, and none with pulse off.
func TestPulseLocalhostCharts(t *testing.T) {
	definitions := func(t *testing.T, opts daemon.Options) *Pair {
		p := StartPair(t, opts, parentIdentity)
		for _, side := range p.Each() {
			waitPulseStored(t, side.Daemon)
		}
		var order [2][]string
		for i, side := range p.Each() {
			order[i] = firstCycle(localCharts(t, side.Daemon))
		}
		if !slices.Equal(order[0], order[1]) {
			t.Errorf("first-cycle charts:\noracle:    %v\ncandidate: %v", order[0], order[1])
		}
		diffs, err := p.CompareJSON("/api/v1/charts", nil, pulseChartsRules)
		if err != nil {
			t.Fatal(err)
		}
		for _, d := range diffs {
			t.Error(d)
		}
		return p
	}
	t.Run("alloc", func(t *testing.T) {
		p := definitions(t, daemon.Options{DBMode: "alloc", StreamMemoryMode: "alloc", StorageTiers: 1})

		// the web API's requests over a window idle on both ends: K requests, as many on each side
		const k = 5
		time.Sleep(3 * time.Second)
		from := time.Now().Unix()
		for _, side := range p.Each() {
			for range k {
				if _, err := rawExchange(side.Daemon.Addr,
					[]byte("GET /api/v1/info HTTP/1.1\r\nConnection: close\r\n\r\n"), 10*time.Second); err != nil {
					t.Fatal(err)
				}
			}
		}
		time.Sleep(3 * time.Second)
		to := time.Now().Unix()
		time.Sleep(2 * time.Second)
		compareLocalData(t, p, "netdata.requests", from, to, "sum")
		if got := localData(t, p.Candidate, "netdata.requests", from, to, "sum"); !strings.HasSuffix(got, ","+strconv.Itoa(k)) {
			t.Errorf("the candidate counted other than %d requests: %s", k, got)
		}

		// a child: the inbound nodes, and its state, one-hot, over settled seconds
		for _, side := range p.Each() {
			conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, childHost, stream.CapsLive)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			t.Cleanup(func() { _ = conn.Close() })
			streamDataFixture(t, conn, time.Now().Unix()/60*60-120)
		}
		time.Sleep(5 * time.Second)
		now := time.Now().Unix()
		for _, chart := range []string{"netdata.netdata.streaming_inbound_permanent",
			"netdata.netdata.streaming_inbound_ephemeral", "netdata.streaming.in.state." + childHost.MachineGUID} {
			compareLocalData(t, p, chart, now-3, now-1, "average")
		}

		// the uptime counts the seconds since each side's first cycle
		for _, side := range p.Each() {
			up := localData(t, side.Daemon, "netdata.uptime", now-1, now-1, "average")
			lines := strings.Split(up, "\n")
			cells := strings.Split(lines[len(lines)-1], ",")
			v, err := strconv.ParseInt(cells[len(cells)-1], 10, 64)
			if err != nil || v < 5 || v > now-side.Daemon.LaunchStartedAt.Unix()+1 {
				t.Errorf("%s: uptime %q at %d", side.Role, up, now-1)
			}
		}
	})
	t.Run("dbengine", func(t *testing.T) {
		p := definitions(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 2})
		// the time retention is 0 under the harness (its `retention time = 0`), every ten seconds: a point on each
		// side, not an empty answer on both
		time.Sleep(25 * time.Second)
		now := time.Now().Unix()
		for tier := range 2 {
			chart := fmt.Sprintf("netdata.dbengine_retention_tier%d&dimensions=time", tier)
			compareLocalData(t, p, chart, now-25, now-1, "max")
			for _, side := range p.Each() {
				if got := localData(t, side.Daemon, chart, now-25, now-1, "max"); !strings.HasSuffix(got, ",0") {
					t.Errorf("%s: tier %d's time retention: %q", side.Role, tier, got)
				}
			}
		}
	})
	t.Run("seeded", func(t *testing.T) {
		// an archived child from the seed: counted stale, and its own state archived
		seed := seedFromOracle(t, parentIdentity, "dbengine")
		p := StartPair(t, daemon.Options{StreamMemoryMode: "dbengine", StorageTiers: 1, SeedCache: seed},
			parentIdentity)
		for _, side := range p.Each() {
			waitPulseStored(t, side.Daemon)
		}
		time.Sleep(4 * time.Second)
		now := time.Now().Unix()
		for _, chart := range []string{"netdata.netdata.streaming_inbound_permanent",
			"netdata.netdata.streaming_inbound_ephemeral", "netdata.streaming.in.state." + childHost.MachineGUID} {
			compareLocalData(t, p, chart, now-3, now-1, "average")
		}
	})
	t.Run("pulse-off", func(t *testing.T) {
		p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, PulseOff: true}, parentIdentity)
		time.Sleep(3 * time.Second)
		for _, side := range p.Each() {
			if ids := localCharts(t, side.Daemon); len(ids) != 0 {
				t.Errorf("%s: localhost charts with pulse off: %v", side.Role, ids)
			}
		}
	})
}
