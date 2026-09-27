// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/binary"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
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

// tierRetention is each metric's retention per tier on host ("chart.dim" -> tiers' "first..last"), from the v1 debug
// plans of every chart over [after, before].
func tierRetention(t *testing.T, d *daemon.Daemon, host string, names map[string]string, after, before int64,
) map[string][]string {
	t.Helper()
	out := map[string][]string{}
	charts := map[string]bool{}
	for _, name := range names {
		charts[name[:strings.LastIndex(name, ".")]] = true
	}
	for chart := range charts {
		path := fmt.Sprintf("/host/%s/api/v1/data?chart=%s&after=%d&before=%d&points=10&options=jsonwrap,debug",
			host, chart, after, before)
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

// The size workload (R2): 200 dimensions of incompressible values, 1 s apart, about 32 MiB of tier-0 files.
const (
	r2Charts, r2Dims = 10, 20
	r2Span           = 40000
)

// liveFirstSpan bounds how far a metric's tier-0 first time may differ between the daemons after live deletions: their
// files end at different pages and their deletion counts may differ by one file, which holds at most two pages (of
// 1024 points, 1 s apart) of each metric (a 512 KiB file holds about 620 points of each of the 200).
const liveFirstSpan = 2 * 1024

var r2child = stream.HostInfo{Hostname: "r2child", MachineGUID: "b6b6b6b6-1111-4111-8111-000000000006"}

// r2value is a pseudo-random value of 9 significant digits (splitmix64 of the point's coordinates), which no page
// encoding compresses.
func r2value(c, d int, t int64) string {
	x := uint64(t)*0x9e3779b97f4a7c15 + uint64(c)<<32 + uint64(d)
	x = (x ^ x>>30) * 0xbf58476d1ce4e5b9
	x = (x ^ x>>27) * 0x94d049bb133111eb
	x ^= x >> 31
	return strconv.FormatFloat(float64(x%1_000_000_000)/1000, 'f', 3, 64)
}

func r2gen() childGen {
	return childGen{host: r2child, prefix: "r2.", charts: r2Charts, dims: r2Dims,
		context: func(int) string { return "r2.ctx" },
		skips:   func(int, int64) bool { return false },
		point:   func(c, d int, t int64) (string, string) { return r2value(c, d, t), stream.FlagNotAnomalous }}
}

// dirSize is the bytes of a directory's files.
func dirSize(t *testing.T, dir string) int64 {
	t.Helper()
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	var size int64
	for _, e := range entries {
		info, err := e.Info()
		if err != nil {
			t.Fatal(err)
		}
		size += info.Size()
	}
	return size
}

// r2seed has the C agent ingest the size workload into one tier of 64 MiB, which keeps all of it, and returns its
// cache and the workload's window.
func r2seed(t *testing.T) (string, int64, int64) {
	t.Helper()
	end := time.Now().Unix()
	start := end - r2Span + 1
	o := daemon.Options{Binary: binaries(t)[0], StorageTiers: 1, TierRetentionMB: [3]int{64}, PulseOff: true,
		RunDir: runDir(t, Role("size-seed")), Identity: &parentIdentity}
	d, err := daemon.Start(o)
	if err != nil {
		t.Fatalf("size seed: %v", err)
	}
	t.Cleanup(func() { _ = d.Stop() })
	if err := r2gen().stream(d, start, end); err != nil {
		t.Fatalf("size seed: streaming: %v", err)
	}
	waitChartsLast(t, d, r2child.Hostname, "r2.", r2Charts, end, 10*time.Minute)
	if err := d.Stop(); err != nil {
		t.Fatalf("size seed: stop: %v", err)
	}
	for _, l := range logLines(t, o.RunDir, "daemon.log") {
		if strings.Contains(l, "deleted datafile") {
			t.Fatalf("size seed: C deleted a file: %s", l)
		}
	}
	cache := filepath.Join(o.RunDir, "cache")
	if size := dirSize(t, filepath.Join(cache, "dbengine")); size < 30<<20 {
		t.Fatalf("size seed: %d bytes in tier 0, want over 30 MiB", size)
	}
	return cache, start, end
}

// waitDeletions waits until a daemon deleted tier-0 files and then none for quiet, and returns how many.
func waitDeletions(t *testing.T, d *daemon.Daemon, quiet, timeout time.Duration) int {
	t.Helper()
	deadline := time.Now().Add(timeout)
	count, since := 0, time.Now()
	for {
		n := 0
		for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
			if strings.Contains(l, "DBENGINE: tier 0: deleted datafile-1-") {
				n++
			}
		}
		if n != count {
			count, since = n, time.Now()
		}
		if count > 0 && time.Since(since) >= quiet {
			return count
		}
		if time.Now().After(deadline) {
			t.Fatalf("%s: %d deletions in %s, still going or none", d.Opts.Binary, count, timeout)
		}
		time.Sleep(time.Second)
	}
}

// retentionDiff lists the metrics whose retention differs, as "name a (b)".
func retentionDiff(a, b map[string][]string) []string {
	var out []string
	for name, tiers := range a {
		if fmt.Sprint(tiers) != fmt.Sprint(b[name]) {
			out = append(out, fmt.Sprintf("%s %v (%v)", name, tiers, b[name]))
		}
	}
	for name, tiers := range b {
		if _, ok := a[name]; !ok {
			out = append(out, fmt.Sprintf("%s missing (%v)", name, tiers))
		}
	}
	sort.Strings(out)
	return out
}

// restarted is every metric's retention per tier on host after each side restarted on a copy of its cache, which the
// two must agree on (C's is returned), and after C started on a copy of each (the handback), which must agree too.
func restarted(t *testing.T, opts daemon.Options, id daemon.Identity, caches [2]string, host string,
	after, before int64,
) map[string][]string {
	t.Helper()
	bins := binaries(t)
	var out map[string][]string
	for _, run := range []struct {
		name string
		bins [2]string
	}{{"restart", bins}, {"handback", [2]string{bins[0], bins[0]}}} {
		t.Run(run.name, func(t *testing.T) {
			r := startPair(t, opts, id, run.bins, copyCaches(t, caches),
				[2]Role{Role(run.name + "-oracle"), Role(run.name + "-candidate")})
			var got [2]map[string][]string
			for i, side := range r.Each() {
				got[i] = tierRetention(t, side.Daemon, host, tierUUIDs(t, side.Daemon, host), after, before)
			}
			for _, d := range retentionDiff(got[1], got[0]) {
				t.Errorf("after the %s: candidate %s, oracle in parentheses", run.name, d)
			}
			if out == nil {
				out = got[0]
			}
		})
	}
	return out
}

// stopAndCompare stops both daemons, compares their tier-0 files and their whole daemon logs, and returns their
// caches.
func stopAndCompare(t *testing.T, p *Pair) [2]string {
	t.Helper()
	caches := stopBoth(t, p)
	if o, c := tier0Files(t, caches[0]), tier0Files(t, caches[1]); fmt.Sprint(o) != fmt.Sprint(c) {
		t.Errorf("tier 0 files:\noracle:    %v\ncandidate: %v", o, c)
	}
	compareLogFiles(t, p, "daemon.log")
	return caches
}

var (
	deletionRecordRe = regexp.MustCompile(`msg="(DBENGINE: tier 0: (?:datafile-1-\d+ (?:is pending|entered deletion)|` +
		`recalculating retention|updating metrics registry|deleting datafile|deleted datafile|partial delete|` +
		`waiting for datafile|datafile-1-\d+ could not be acquired)[^"]*)"`)
	deletedFileRe = regexp.MustCompile(`deleted datafile-1-(\d+) `)
	numbersRe     = regexp.MustCompile(`\d+(\.\d+)?`)
)

// deletions are a daemon's tier-0 deletion records, grouped per deleted file (each group ends with its "deleted"
// record), and the deleted file numbers.
func deletions(t *testing.T, d *daemon.Daemon) ([][]string, []int) {
	t.Helper()
	var groups [][]string
	var files []int
	var group []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		m := deletionRecordRe.FindStringSubmatch(l)
		if m == nil {
			continue
		}
		group = append(group, m[1])
		if f := deletedFileRe.FindStringSubmatch(m[1]); f != nil {
			n, _ := strconv.Atoi(f[1])
			files = append(files, n)
			groups = append(groups, group)
			group = nil
		}
	}
	if group != nil {
		groups = append(groups, group)
	}
	return groups, files
}

// TestDbengineRetention (check `dbengine.retention`, env-gated like `dbengine.read`): the dbengine's deletion of a
// tier's oldest files against C's.
//   - time: both daemons on copies of the runR cache with tier 0's retention time ending midway between the data of
//     its first two files, which the first retention check (60 s after the start) finds over the cap: file 1 goes
//     and no other. Compared: the files left, every metric's retention per tier (the D30 victim's tier-0 first time
//     is C's walk against the exact one, asserted apart), the whole daemon log; then a restart on each cache and C
//     on the candidate's cache, which rebuild the registry from the files left (the victim then equal too).
//   - size: C ingests about 32 MiB of incompressible data into one tier of 64 MiB (nothing deleted), then both
//     daemons start on copies of its cache with 25 MiB and delete the oldest files until under the cap. Compared as
//     time; the candidate's retention equals what the files left give after a restart, and C's differs from the
//     candidate's only where its walk dated a metric from a later file (the D30 victims, logged).
//   - live: the size workload streamed into both daemons with 25 MiB, fresh (structural: the file boundaries
//     differ). Each deletes its oldest files in order and keeps 3 pairs at least; the deletion counts are within one;
//     each deletion's records equal with their numbers masked; every metric's tier-0 first time within two pages, its
//     last time equal.
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
			retention[i] = tierRetention(t, side.Daemon, "b6child", names[i], fixtureStart, fixtureEnd)
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
		caches := stopAndCompare(t, p)
		restarted(t, opts, id, caches, "b6child", fixtureStart, fixtureEnd)
	})
	t.Run("size", func(t *testing.T) {
		seed, start, end := r2seed(t)
		opts := daemon.Options{StorageTiers: 1, TierRetentionMB: [3]int{25}, SeedCache: seed, PulseOff: true,
			LogsExtra: "    level = debug\n"}
		p := StartPair(t, opts, parentIdentity)
		var deleted [2]int
		for i, side := range p.Each() {
			deleted[i] = waitDeletions(t, side.Daemon, 15*time.Second, 180*time.Second)
		}
		if deleted[0] != deleted[1] {
			t.Errorf("deletions: oracle %d, candidate %d", deleted[0], deleted[1])
		}
		var live [2]map[string][]string
		for i, side := range p.Each() {
			live[i] = tierRetention(t, side.Daemon, r2child.Hostname, tierUUIDs(t, side.Daemon, r2child.Hostname),
				start, end)
		}
		caches := stopAndCompare(t, p)
		exact := restarted(t, opts, parentIdentity, caches, r2child.Hostname, start, end)
		for _, d := range retentionDiff(live[1], exact) {
			t.Errorf("candidate: after the deletions %s, not what the files left give", d)
		}
		var victims []string
		for name, tiers := range live[0] {
			if fmt.Sprint(tiers) == fmt.Sprint(live[1][name]) {
				continue
			}
			if fmt.Sprint(tiers) == fmt.Sprint(exact[name]) {
				t.Errorf("%s: oracle %v, candidate %v", name, tiers, live[1][name])
				continue
			}
			victims = append(victims, fmt.Sprintf("%s %v (exact %v)", name, tiers, exact[name]))
		}
		sort.Strings(victims)
		t.Logf("%d deletions; metrics C's walk dated from later files (D30): %v", deleted[0], victims)
	})
	t.Run("live", func(t *testing.T) {
		end := time.Now().Unix()
		start := end - r2Span + 1
		opts := daemon.Options{StorageTiers: 1, TierRetentionMB: [3]int{25}, PulseOff: true,
			LogsExtra: "    level = debug\n"}
		p := StartPair(t, opts, parentIdentity)
		r2gen().streamBoth(t, p, start, end)
		var groups [2][][]string
		var files [2][]int
		for i, side := range p.Each() {
			waitDeletions(t, side.Daemon, 15*time.Second, 180*time.Second)
			groups[i], files[i] = deletions(t, side.Daemon)
			// the oldest files, in order
			for j, f := range files[i] {
				if f != j+1 {
					t.Errorf("%s: deleted files %v, want 1, 2, ...", side.Role, files[i])
					break
				}
			}
			// under the cap (C's estimate adds a whole file less the last one's position), 2 files at least
			dir := filepath.Join(side.Daemon.Opts.RunDir, "cache", "dbengine")
			datafiles, _ := filepath.Glob(filepath.Join(dir, "datafile-1-*.ndf"))
			if size := dirSize(t, dir); len(datafiles) < 2 || size > 25<<20+1<<20 {
				t.Errorf("%s: tier 0 keeps %d data files, %d bytes", side.Role, len(datafiles), size)
			}
		}
		t.Logf("deletions: oracle %d, candidate %d", len(files[0]), len(files[1]))
		if n := len(files[0]) - len(files[1]); n < -1 || n > 1 {
			t.Errorf("deletions: oracle %v, candidate %v", files[0], files[1])
		}
		// each deletion's records, their numbers masked (file numbers, metric counts, sizes)
		for j := 0; j < min(len(groups[0]), len(groups[1])); j++ {
			o, c := groups[0][j], groups[1][j]
			mask := func(g []string) string { return numbersRe.ReplaceAllString(strings.Join(g, "\n"), "N") }
			if mask(o) != mask(c) {
				t.Errorf("deletion %d:\noracle:\n%s\ncandidate:\n%s", j+1, strings.Join(o, "\n"), strings.Join(c, "\n"))
			}
		}
		// every metric's tier-0 retention within a page of each other: the file boundaries differ (D65.1)
		var retention [2]map[string][]string
		for i, side := range p.Each() {
			retention[i] = tierRetention(t, side.Daemon, r2child.Hostname, tierUUIDs(t, side.Daemon, r2child.Hostname),
				start, end)
		}
		if len(retention[0]) != len(retention[1]) {
			t.Errorf("metrics: oracle %d, candidate %d", len(retention[0]), len(retention[1]))
		}
		for name, tiers := range retention[0] {
			other, ok := retention[1][name]
			if !ok || len(tiers) == 0 || len(other) == 0 {
				t.Errorf("%s: oracle %v, candidate %v", name, tiers, other)
				continue
			}
			var of, ol, cf, cl int64
			fmt.Sscanf(tiers[0], "%d..%d", &of, &ol)
			fmt.Sscanf(other[0], "%d..%d", &cf, &cl)
			if d := of - cf; d < -liveFirstSpan || d > liveFirstSpan || ol != cl {
				t.Errorf("%s: tier 0 oracle %s, candidate %s", name, tiers[0], other[0])
			}
		}
		// no restart check: the stop flushes a page of every metric, which takes one more file (C deletes during its
		// shutdown too), so the restarted daemons read other files than the live ones; `size` proves exactness
	})
}
