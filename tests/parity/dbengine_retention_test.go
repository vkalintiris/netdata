// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/binary"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// d30Victim is the runR metric C's first-time recalculation skips when tier 0's file 1 goes: the last entry of file
// 2's metric list, which C then dates from file 3 (C's walk records a match only when it is not a list's last entry);
// the port records it (the user's D30, D75.3).
const (
	d30Victim      = "d00ed4f3663847c9837a665030281d48"
	d30VictimC     = 1790192784
	d30VictimExact = 1790086288
)

// v2End is the data end time of a tier-0 v2 index of the runR fixture (its header's end, in seconds).
func v2End(t *testing.T, fx string, fileno int) int64 {
	t.Helper()
	b, err := os.ReadFile(filepath.Join(fx, "runR", "cache", "dbengine", fmt.Sprintf("journalfile-1-%010d.njfv2", fileno)))
	if err != nil || len(b) < 24 {
		t.Fatalf("v2 header of file %d: %v", fileno, err)
	}
	return int64(binary.LittleEndian.Uint64(b[16:24]) / 1_000_000)
}

// waitRecord waits until the daemon's log has a record containing text.
func waitRecord(t *testing.T, d *daemon.Daemon, text string, timeout time.Duration) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for {
		for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
			if strings.Contains(l, text) {
				return
			}
		}
		if time.Now().After(deadline) {
			t.Fatalf("%s: no %q record in %s", d.Opts.Binary, text, timeout)
		}
		time.Sleep(time.Second)
	}
}

// tierRetention is each b6child metric's retention per tier ("chart.dim" -> tiers' "first..last"), from the v1 debug
// plans of every chart.
func tierRetention(t *testing.T, d *daemon.Daemon, names map[string]string) map[string][]string {
	t.Helper()
	out := map[string][]string{}
	charts := map[string]bool{}
	for _, name := range names {
		charts[name[:strings.LastIndex(name, ".")]] = true
	}
	for chart := range charts {
		path := fmt.Sprintf("/host/b6child/api/v1/data?chart=%s&after=%d&before=%d&points=10&options=jsonwrap,debug",
			chart, fixtureStart, fixtureEnd)
		var plans map[string]struct {
			Tiers []struct {
				First int64 `json:"fe"`
				Last  int64 `json:"le"`
			} `json:"tiers"`
		}
		if err := json.Unmarshal([]byte(member(t, d, path, "query_plan")), &plans); err != nil {
			t.Fatalf("%s: %v", path, err)
		}
		for dim, p := range plans {
			var tiers []string
			for _, tr := range p.Tiers {
				tiers = append(tiers, fmt.Sprintf("%d..%d", tr.First, tr.Last))
			}
			out[chart+"."+dim] = tiers
		}
	}
	return out
}

// tier0Files are the file names left in a cache's tier 0.
func tier0Files(t *testing.T, cache string) []string {
	t.Helper()
	entries, err := os.ReadDir(filepath.Join(cache, "dbengine"))
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, e := range entries {
		names = append(names, e.Name())
	}
	sort.Strings(names)
	return names
}

// TestDbengineRetention (check `dbengine.retention`, env-gated like `dbengine.read`): the dbengine's deletion of a
// tier's oldest files against C's.
//   - time: both daemons on copies of the runR cache with tier 0's retention time ending midway between the data of
//     its first two files, which the first retention check (60 s after the start) finds over the cap: file 1 goes
//     and no other. Compared: the files left, every metric's retention per tier (the D30 victim's tier-0 first time
//     is C's walk against the exact one, asserted apart), the whole daemon log; then a restart on each cache and C
//     on the candidate's cache, which rebuild the registry from the files left (the victim then equal too).
func TestDbengineRetention(t *testing.T) {
	fx, id := runRParent(t)
	t.Run("time", func(t *testing.T) {
		cutoff := (v2End(t, fx, 1) + v2End(t, fx, 2)) / 2
		opts := daemon.Options{StorageTiers: 3, TierRetentionMB: [3]int{25, 25, 25},
			TierRetentionTime: [3]string{fmt.Sprintf("%ds", time.Now().Unix()-cutoff)},
			SeedCache:         filepath.Join(fx, "runR", "cache"), PulseOff: true, LogsExtra: "    level = debug\n"}
		p := StartPair(t, opts, id)
		for _, side := range p.Each() {
			waitRecord(t, side.Daemon, "deleted datafile-1-0000000001", 150*time.Second)
		}
		var names [2]map[string]string
		var retention [2]map[string][]string
		for i, side := range p.Each() {
			names[i] = tierUUIDs(t, side.Daemon, "b6child")
			retention[i] = tierRetention(t, side.Daemon, names[i])
		}
		victim := names[0][d30Victim]
		if victim == "" {
			t.Fatalf("the D30 victim %s is not a metric of b6child", d30Victim)
		}
		first := func(r map[string][]string) string { return strings.Split(r[victim][0], "..")[0] }
		if got, want := first(retention[0]), fmt.Sprint(d30VictimC); got != want {
			t.Errorf("oracle: %s tier 0 starts at %s, want C's %s", victim, got, want)
		}
		if got, want := first(retention[1]), fmt.Sprint(d30VictimExact); got != want {
			t.Errorf("candidate: %s tier 0 starts at %s, want the exact %s", victim, got, want)
		}
		for name, tiers := range retention[0] {
			other := retention[1][name]
			if name == victim {
				tiers, other = tiers[1:], other[1:]
			}
			if fmt.Sprint(tiers) != fmt.Sprint(other) {
				t.Errorf("%s: oracle %v, candidate %v", name, tiers, other)
			}
		}
		if len(retention[0]) != len(retention[1]) {
			t.Errorf("metrics: oracle %d, candidate %d", len(retention[0]), len(retention[1]))
		}
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		caches := [2]string{filepath.Join(p.Oracle.Opts.RunDir, "cache"), filepath.Join(p.Candidate.Opts.RunDir, "cache")}
		if o, c := tier0Files(t, caches[0]), tier0Files(t, caches[1]); fmt.Sprint(o) != fmt.Sprint(c) {
			t.Errorf("tier 0 files:\noracle:    %v\ncandidate: %v", o, c)
		}
		compareLogFiles(t, p, "daemon.log")
		for _, run := range []struct {
			name string
			bins [2]string
		}{{"restart", binaries(t)}, {"handback", [2]string{binaries(t)[0], binaries(t)[0]}}} {
			t.Run(run.name, func(t *testing.T) {
				r := startPair(t, opts, id, run.bins, copyCaches(t, caches),
					[2]Role{Role(run.name + "-oracle"), Role(run.name + "-candidate")})
				var after [2]map[string][]string
				for i, side := range r.Each() {
					after[i] = tierRetention(t, side.Daemon, tierUUIDs(t, side.Daemon, "b6child"))
				}
				if fmt.Sprint(after[0]) != fmt.Sprint(after[1]) {
					t.Errorf("retention after the %s:\noracle:    %v\ncandidate: %v", run.name, after[0], after[1])
				}
			})
		}
	})
}
