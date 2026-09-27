// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
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
	if !bytes.HasPrefix(b, []byte("HTTP/1.1 200 ")) || len(lines) < 2 {
		t.Fatalf("%s: %s [%d, %d] %s: no points: %q", d.Opts.Binary, chart, after, before, group, truncateBytes(b))
	}
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

// lastValue is the last cell of localData's answer.
func lastValue(t *testing.T, csv string) int64 {
	t.Helper()
	lines := strings.Split(csv, "\n")
	cells := strings.Split(lines[len(lines)-1], ",")
	v, err := strconv.ParseInt(cells[len(cells)-1], 10, 64)
	if err != nil {
		t.Fatalf("%q: %v", csv, err)
	}
	return v
}

// jsonNumber is a top-level number member of a JSON answer.
func jsonNumber(t *testing.T, d *daemon.Daemon, path, name string) int64 {
	t.Helper()
	b, err := rawExchange(d.Addr, []byte("GET "+path+" HTTP/1.1\r\nConnection: close\r\n\r\n"), 10*time.Second)
	if err != nil {
		t.Fatalf("%s: %v", d.Opts.Binary, err)
	}
	dec := json.NewDecoder(bytes.NewReader(httpBody(b)))
	dec.UseNumber()
	var raw map[string]any
	if err := dec.Decode(&raw); err != nil {
		t.Fatalf("%s: %s: %v", d.Opts.Binary, path, err)
	}
	n, ok := raw[name].(json.Number)
	if !ok {
		t.Fatalf("%s: %s: no number %s", d.Opts.Binary, path, name)
	}
	v, err := n.Int64()
	if err != nil {
		t.Fatalf("%s: %s: %s: %v", d.Opts.Binary, path, name, err)
	}
	return v
}

// pulseContextsRules compare localhost's /api/v1/contexts: every context's definition, charts, dimensions and
// labels, in any context order, the clocks masked.
var pulseContextsRules = Rules{
	Masks: []Mask{
		{Pattern: "**.first_time_t", Reason: "each daemon's own start"},
		{Pattern: "**.last_time_t", Reason: "the clock"},
		{Pattern: "host_labels", Reason: "compared by api.localhost-identity (C's _hw_* labels, D49.3)"},
	},
	// C keeps a chart's labels in pointer order (D22.2)
	Unordered: []string{"contexts", "contexts.*.charts.*.labels"},
	// the contexts follow their charts through RRDCONTEXT's queue
	Settle: 10 * time.Second,
}

// localCharts are localhost's chart ids in /api/v1/charts' order.
func localCharts(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	ids, _, err := hostCharts(d, "")
	if err != nil {
		t.Fatalf("%s: %v", d.Opts.Binary, err)
	}
	return ids
}

// firstCycle drops the lazy charts from ids.
func firstCycle(ids []string) []string {
	return slices.DeleteFunc(slices.Clone(ids), func(id string) bool { return lazyPulse.MatchString(id) })
}

// lazyOnly keeps the lazy charts of ids, sorted.
func lazyOnly(ids []string) []string {
	lazy := slices.DeleteFunc(slices.Clone(ids), func(id string) bool { return !lazyPulse.MatchString(id) })
	slices.Sort(lazy)
	return lazy
}

// waitLocalCharts waits, within timeout, until localhost has every chart of ids on both sides.
func waitLocalCharts(t *testing.T, p *Pair, timeout time.Duration, ids ...string) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for _, side := range p.Each() {
		for {
			_, charts, err := hostCharts(side.Daemon, "")
			missing := slices.DeleteFunc(slices.Clone(ids), func(id string) bool {
				_, ok := charts[id]
				return err == nil && ok
			})
			if len(missing) == 0 {
				break
			}
			if time.Now().After(deadline) {
				t.Fatalf("%s: localhost lacks %v (%v)", side.Role, missing, err)
			}
			time.Sleep(200 * time.Millisecond)
		}
	}
}

// waitLazyEqual waits until both sides have the same lazy charts.
func waitLazyEqual(t *testing.T, p *Pair) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for {
		var lazy [2][]string
		for i, side := range p.Each() {
			lazy[i] = lazyOnly(localCharts(t, side.Daemon))
		}
		if slices.Equal(lazy[0], lazy[1]) {
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("lazy pulse charts:\noracle:    %v\ncandidate: %v", lazy[0], lazy[1])
		}
		time.Sleep(200 * time.Millisecond)
	}
}

// childPulseCharts are a child's four inbound charts on its parent.
func childPulseCharts(guid string) []string {
	var ids []string
	for _, kind := range []string{"traffic", "state", "reconnects", "age"} {
		ids = append(ids, "netdata.streaming.in."+kind+"."+guid)
	}
	return ids
}

// compareLazyDefinitions waits until both sides have the lazy charts that data queries, a child (with its four) and,
// while it streams, its traffic create, then compares every definition again (R29 B1).
func compareLazyDefinitions(t *testing.T, p *Pair, guid string, streaming bool) {
	t.Helper()
	ids := append([]string{"netdata.network_api", "netdata.db_points_results"}, childPulseCharts(guid)...)
	if streaming {
		ids = append(ids, "netdata.network_streaming")
	}
	waitLocalCharts(t, p, 10*time.Second, ids...)
	comparePulseDefinitions(t, p)
}

// comparePulseDefinitions compares localhost's charts and contexts on both sides.
func comparePulseDefinitions(t *testing.T, p *Pair) {
	t.Helper()
	for path, rules := range map[string]Rules{
		"/api/v1/charts": pulseChartsRules,
		"/api/v1/contexts?options=charts,dimensions,labels": pulseContextsRules,
	} {
		diffs, err := p.CompareJSON(path, nil, rules)
		if err != nil {
			t.Fatal(err)
		}
		for _, d := range diffs {
			t.Errorf("%s: %s", path, d)
		}
	}
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
		comparePulseDefinitions(t, p)
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
		compareLazyDefinitions(t, p, childHost.MachineGUID, true)

		// the uptime counts the seconds since each side's first stored point: the stored span, as C's (R29 M5)
		for _, side := range p.Each() {
			v := lastValue(t, localData(t, side.Daemon, "netdata.uptime", now-1, now-1, "average"))
			first := jsonNumber(t, side.Daemon, "/api/v1/chart?chart=netdata.uptime", "first_entry")
			if off := v - (now - 1 - first); off != 0 {
				t.Errorf("%s: uptime %d at %d, its first entry %d: %d off the stored span", side.Role, v, now-1, first,
					off)
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
	t.Run("dbengine-retention", func(t *testing.T) {
		// tier 0 with a 120 s time limit: its time retention is the age of the tier's first time in percent of the
		// limit, each side's first time the second its fresh tier 0 became ready (R29 M4)
		const limit = 120
		p := definitions(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1,
			TierRetentionTime: [3]string{fmt.Sprintf("%ds", limit)}})
		time.Sleep(25 * time.Second)
		const chart = "netdata.dbengine_retention_tier0"
		for _, side := range p.Each() {
			ready := int64(0)
			for _, l := range logLines(t, side.Daemon.Opts.RunDir, "daemon.log") {
				if strings.Contains(l, `msg="DBENGINE: tier 0: ready for data collection and queries"`) {
					at, err := time.Parse(time.RFC3339Nano, strings.TrimPrefix(strings.Fields(l)[0], "time="))
					if err != nil {
						t.Fatalf("%s: %q: %v", side.Role, l, err)
					}
					ready = at.Unix()
				}
			}
			if ready == 0 {
				t.Fatalf("%s: no tier 0 readiness record", side.Role)
			}
			last := jsonNumber(t, side.Daemon, "/api/v1/chart?chart="+chart, "last_entry")
			v := lastValue(t, localData(t, side.Daemon, chart+"&dimensions=time", last-9, last, "max"))
			want := min(100, 100*(last-ready)/limit)
			if v < 1 || v < want-1 || v > want+1 {
				t.Errorf("%s: time retention %d at %d, ready at %d: expected %d", side.Role, v, last, ready, want)
			}
		}
	})
	t.Run("dbengine-cchild", func(t *testing.T) {
		// a real C child through the tee: its states as both parents follow it, online, stopped, then marked
		// ephemeral
		const hostname, guid = "parity-cchild-pulse", "5a1e0000-0000-4000-8000-00000000c0ab"
		p := definitions(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1})
		child := startCChild(t, Role("child"), hostname, guid, startTee(t, p, &teeReplies{}), false)
		states := func(phase string) {
			t.Helper()
			now := time.Now().Unix()
			for _, chart := range []string{"netdata.netdata.streaming_inbound_permanent",
				"netdata.netdata.streaming_inbound_ephemeral", "netdata.streaming.in.state." + guid} {
				t.Run(phase, func(t *testing.T) { compareLocalData(t, p, chart, now-3, now-1, "average") })
			}
		}
		for _, side := range p.Each() {
			waitOnline(t, side.Daemon.Addr, guid)
		}
		states("online")
		compareLazyDefinitions(t, p, guid, true)
		if err := child.Stop(); err != nil {
			t.Fatalf("stop child: %v", err)
		}
		time.Sleep(5 * time.Second)
		states("stopped")
		for _, side := range p.Each() {
			if r := runCLI(t, side.Daemon, "mark-stale-nodes-ephemeral", hostname); r.Exit != 0 {
				t.Fatalf("%s: mark-stale-nodes-ephemeral %+v", side.Role, r)
			}
		}
		time.Sleep(5 * time.Second)
		states("ephemeral")
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
		// the archived child's charts take its labels from the seed's SQLite
		compareLazyDefinitions(t, p, childHost.MachineGUID, false)
	})
	t.Run("pulse-off", func(t *testing.T) {
		p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, PulseOff: true}, parentIdentity)
		time.Sleep(3 * time.Second)
		for _, side := range p.Each() {
			if ids := localCharts(t, side.Daemon); len(ids) != 0 {
				t.Errorf("%s: localhost charts with pulse off: %v", side.Role, ids)
			}
			if n := jsonNumber(t, side.Daemon, "/api/v1/charts", "charts_count"); n != 0 {
				t.Errorf("%s: charts_count %d with pulse off", side.Role, n)
			}
		}
	})
}
