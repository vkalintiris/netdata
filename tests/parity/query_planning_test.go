// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/json"
	"fmt"
	"path/filepath"
	"strconv"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The S4b ram child (D74.4): on a three-tier parent a ram child's tier 0 is a ring and its tiers 1 and 2 are dbengine
// rollups, so a window across the ring's start plans a coarser head before tier 0. Seven charts of two dimensions
// every second for six hours: q.c0 dense; q.c1 flat; q.c2 negative (for absolute); q.c3 a rate (incremental); q.c4
// anomalous every 13 s; q.c5 starting two hours late (later rollups, a valid tier 2); q.c6 starting in the last hour
// (no tier-2 record, so tier=2 plans automatically).
const (
	qpCharts, qpDims = 7, 2
	qpSpan           = 21600
	// qpOff is the end's offset in its hour, so every run's seams land at the same offsets of the tier windows.
	qpOff = 1850
	// qpRing is the stream.conf retention 5000, which both agents align up to whole 4 KiB pages of 1024 slots.
	qpRing = 5120
)

var qpchild = stream.HostInfo{Hostname: "qpchild", MachineGUID: "b6b6b6b6-1111-4111-8111-000000000005"}

type qpgen struct{ start, end int64 }

func (g qpgen) child() childGen {
	lastHour := g.end/3600*3600 + 50
	return childGen{host: qpchild, prefix: "q.", charts: qpCharts, dims: qpDims,
		context: func(c int) string { return fmt.Sprintf("q.ctx%d", c) },
		algorithm: func(c int) string {
			if c == 3 {
				return "incremental"
			}
			return "absolute"
		},
		skips: func(c int, t int64) bool { return c == 5 && t < g.start+7200 || c == 6 && t <= lastHour },
		point: func(c, d int, t int64) (string, string) {
			v := s3value(c, d, t)
			switch c {
			case 1:
				v = float64(7 + d)
			case 2:
				v = float64(-7 - d)
			case 3:
				v = float64(5 + d)
			}
			flags := stream.FlagNotAnomalous
			if c == 4 && t%13 == 0 {
				flags = stream.FlagAnomalous
			}
			return strconv.FormatFloat(v, 'f', -1, 64), flags
		}}
}

// qpRetention is a context's retention per tier, as v3 reports it.
type qpRetention []struct {
	First int64 `json:"first_entry"`
	Last  int64 `json:"last_entry"`
}

// retention asks both daemons for q.ctx1's retention per tier and requires them to agree (the answers compare in
// the subtests).
func (g qpgen) retention(t *testing.T, p *Pair) qpRetention {
	t.Helper()
	path := fmt.Sprintf("/host/%s/api/v3/data?contexts=q.ctx1&after=%d&before=%d&points=1", qpchild.Hostname, g.start,
		g.end)
	var got [2]qpRetention
	for i, side := range p.Each() {
		var db struct {
			PerTier qpRetention `json:"per_tier"`
		}
		doc := member(t, side.Daemon, path, "db")
		if err := json.Unmarshal([]byte(doc), &db); err != nil || len(db.PerTier) != 3 {
			t.Fatalf("%s: retention: %v %s", side.Role, err, doc)
		}
		got[i] = db.PerTier
	}
	if fmt.Sprint(got[0]) != fmt.Sprint(got[1]) {
		t.Fatalf("retention differs: oracle %v, candidate %v", got[0], got[1])
	}
	return got[0]
}

// waitArchived waits until each daemon's contexts worker has archived the disconnected child: a metric still
// collected reports the wall clock as its last entry, and each agent archives at its worker's next tick.
func (g qpgen) waitArchived(t *testing.T, p *Pair) {
	t.Helper()
	path := fmt.Sprintf("/host/%s/api/v3/data?contexts=q.*&after=%d&before=%d&points=1&options=details&harness=wait",
		qpchild.Hostname, g.start, g.end)
	for _, side := range p.Each() {
		deadline := time.Now().Add(30 * time.Second)
		for {
			var detailed struct {
				Nodes map[string]struct {
					Contexts map[string]struct {
						Instances map[string]struct {
							Dimensions map[string]struct {
								Last int64 `json:"le"`
							} `json:"dimensions"`
						} `json:"instances"`
					} `json:"contexts"`
				} `json:"nodes"`
			}
			archived, n := true, 0
			if err := json.Unmarshal([]byte(member(t, side.Daemon, path, "detailed")), &detailed); err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			for _, node := range detailed.Nodes {
				for _, c := range node.Contexts {
					for _, i := range c.Instances {
						for _, d := range i.Dimensions {
							n++
							archived = archived && d.Last == g.end
						}
					}
				}
			}
			if archived && n == qpCharts*qpDims {
				break
			}
			if time.Now().After(deadline) {
				t.Fatalf("%s: the child's %d metrics were not archived at %d", side.Role, n, g.end)
			}
			time.Sleep(200 * time.Millisecond)
		}
	}
}

// qpWindow is a window of the check: its bounds and points.
type qpWindow struct {
	name          string
	after, before int64
	points        int
}

// windows are the windows the check reads, from the reported retention: F is tier 0's first entry (the ring's start).
func (g qpgen) windows(f int64) []qpWindow {
	e := g.end
	return []qpWindow{
		{"head-1s", f - 299, f + 300, 600},
		{"head-10s", f - 2999, f + 3000, 600},
		{"head-30s", f - 2999, f + 3000, 200},
		{"flip-31s", f - 3099, f + 3100, 200},
		{"across-60s", f - 2999, f + 3000, 100},
		{"across-600s", e - 11999, e, 20},
		{"tail-60s", e - 3599, e, 60},
		{"whole-1800", e - 21599, e, 12},
		{"whole-1801", e - 21611, e, 12},
		{"whole", g.start, e, 6},
		{"before-ring-1s", f - 900, f - 301, 600},
		{"before-ring-60s", f - 7259, f - 60, 120},
		{"inside-ring-1s", e - 1299, e - 700, 600},
	}
}

// plannedTiers is the oracle's plan of q.c0's first dimension over a window, as tiers.
func plannedTiers(t *testing.T, p *Pair, w qpWindow) []int {
	t.Helper()
	path := fmt.Sprintf("/host/%s/api/v1/data?chart=q.c0&after=%d&before=%d&points=%d&options=jsonwrap,debug",
		qpchild.Hostname, w.after, w.before, w.points)
	var plan map[string]struct {
		Plans []struct {
			Tier int `json:"tr"`
		} `json:"plans"`
	}
	if err := json.Unmarshal([]byte(member(t, p.Oracle, path, "query_plan")), &plan); err != nil {
		t.Fatal(err)
	}
	var tiers []int
	for _, e := range plan["d0"].Plans {
		tiers = append(tiers, e.Tier)
	}
	return tiers
}

// TestQueryPlanning (check `query.planning`) streams the ram child into both parents, three tiers each, the C agent
// with its pulse charts off, and compares the answers the planner gives across the ring's start (tier 0 with a coarser
// head), before it (tier 1 alone), at the thresholds of the tier choice and over the whole span (tier 2 with finer
// tails): values, the plans and weights, the reads per tier, for every grouping, absolute, a rate's sum and the
// anomaly rates; then tiers 1 and 2 forced, and every tier record, the first comparison of a ram host's rollups.
func TestQueryPlanning(t *testing.T) {
	opts := daemon.Options{StorageTiers: 3, TierRetentionMB: [3]int{25, 25, 25}, PulseOff: true,
		StreamMemoryMode: "ram", LogsExtra: "    level = debug\n",
		StreamExtra: "\n[" + qpchild.MachineGUID + "]\n    retention = 5000\n"}
	p := StartPair(t, opts, parentIdentity)
	end := (time.Now().Unix()-3600-qpOff)/3600*3600 + qpOff
	g := qpgen{start: end - qpSpan + 1, end: end}
	g.child().streamBoth(t, p, g.start, end)
	g.waitArchived(t, p)
	r := g.retention(t, p)
	if r[0].First != end-qpRing || r[0].Last != end {
		t.Fatalf("tier 0 (the ring) holds %d..%d, want %d..%d", r[0].First, r[0].Last, end-qpRing, end)
	}
	var names [2]map[string]string
	for i, side := range p.Each() {
		names[i] = tierUUIDs(t, side.Daemon, qpchild.Hostname)
	}
	host := "/host/" + qpchild.Hostname
	rules := dbengineWriteRules(false)
	windows := g.windows(r[0].First)

	// the windows give the plans they are for: a head at 30 s rows and not at 31 s, tier 2 from 1801 s rows
	t.Run("shapes", func(t *testing.T) {
		want := map[string][]int{"head-1s": {1, 0}, "head-30s": {1, 0}, "flip-31s": {1}, "whole-1800": {1, 0},
			"whole-1801": {2, 1, 0}, "before-ring-1s": {1}, "inside-ring-1s": {0}}
		for _, w := range windows {
			if tiers, ok := want[w.name]; ok {
				if got := plannedTiers(t, p, w); fmt.Sprint(got) != fmt.Sprint(tiers) {
					t.Errorf("%s: the oracle plans tiers %v, want %v", w.name, got, tiers)
				}
			}
		}
	})
	t.Run("auto", func(t *testing.T) {
		for _, w := range windows {
			win := fmt.Sprintf("after=%d&before=%d&points=%d", w.after, w.before, w.points)
			var paths []string
			for _, group := range []string{"average", "sum", "min", "max"} {
				paths = append(paths,
					"/api/v1/data?chart=q.c0&"+win+"&group="+group+"&options=jsonwrap,debug",
					"/api/v3/data?contexts=q.*&"+win+"&time_group="+group+"&options=debug,details")
			}
			paths = append(paths,
				"/api/v1/data?chart=q.c1&"+win+"&options=jsonwrap,debug",
				"/api/v1/data?chart=q.c2&"+win+"&options=jsonwrap,debug,absolute",
				"/api/v1/data?chart=q.c2&"+win+"&group=sum&options=jsonwrap,debug,absolute",
				"/api/v1/data?chart=q.c3&"+win+"&options=jsonwrap,debug",
				"/api/v1/data?chart=q.c3&"+win+"&group=sum&options=jsonwrap,debug",
				"/api/v1/data?chart=q.c4&"+win+"&options=jsonwrap,debug,anomaly-bit",
				"/api/v1/data?chart=q.c5&"+win+"&options=jsonwrap,debug",
				"/api/v1/data?chart=q.c6&"+win+"&options=jsonwrap,debug")
			for _, path := range paths {
				t.Run(w.name+" "+path, func(t *testing.T) { compareGetWith(t, p, host+path, rules) })
			}
		}
	})
	// tier=2 of q.c6, which has no tier-2 record, plans automatically; q.c5's tier 2 serves alone from its later start
	t.Run("forced", func(t *testing.T) {
		for _, w := range windows {
			if w.name != "across-600s" && w.name != "tail-60s" && w.name != "whole" {
				continue
			}
			for tier := 1; tier <= 2; tier++ {
				win := fmt.Sprintf("after=%d&before=%d&points=%d&tier=%d", w.after, w.before, w.points, tier)
				paths := []string{"/api/v3/data?contexts=q.*&" + win + "&options=debug,details"}
				for c := 0; c < qpCharts; c++ {
					paths = append(paths, fmt.Sprintf("/api/v1/data?chart=q.c%d&%s&options=jsonwrap,debug", c, win))
				}
				for _, path := range paths {
					t.Run(w.name+" "+path, func(t *testing.T) { compareGetWith(t, p, host+path, rules) })
				}
			}
		}
	})
	for _, side := range p.Each() {
		if err := side.Daemon.Stop(); err != nil {
			t.Fatalf("stop %s: %v", side.Role, err)
		}
	}
	caches := [2]string{filepath.Join(p.Oracle.Opts.RunDir, "cache"), filepath.Join(p.Candidate.Opts.RunDir, "cache")}
	t.Run("records", func(t *testing.T) { compareTierRecords(t, caches, names) })
}
