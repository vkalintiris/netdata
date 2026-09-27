// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/binary"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The contexts GC workload: gcchild's context gc.x holds the stable chart gc.c0 and the ephemeral gc.c1, its context
// gc.y only the ephemeral gc.c2 and gc.c3, each with gcStableDims dimensions of incompressible values. The ephemerals
// send the first gcEphemeral seconds, which fit tier 0's file 1 (a page of each dimension) and a stop flushes there;
// then only gc.c0 sends, until tier 0 has 4 files at least.
const (
	gcStableDims = 20
	gcEphemeral  = 300
	gcSpan       = 26000
)

var gcchild = stream.HostInfo{Hostname: "gcchild", MachineGUID: "b6b6b6b6-1111-4111-8111-000000000007"}

func gcgen(charts int) childGen {
	return childGen{host: gcchild, prefix: "gc.", charts: charts, dims: gcStableDims,
		context: func(c int) string {
			if c < 2 {
				return "gc.x"
			}
			return "gc.y"
		},
		skips: func(int, int64) bool { return false },
		point: func(c, d int, t int64) (string, string) { return r2value(c, d, t), stream.FlagNotAnomalous }}
}

// v2EndIn is the data end time of a tier-0 v2 index in a cache (its header's end, in seconds).
func v2EndIn(t *testing.T, cache string, fileno int) int64 {
	t.Helper()
	b, err := os.ReadFile(filepath.Join(cache, "dbengine", fmt.Sprintf("journalfile-1-%010d.njfv2", fileno)))
	if err != nil || len(b) < 24 {
		t.Fatalf("v2 header of file %d: %v", fileno, err)
	}
	return int64(binary.LittleEndian.Uint64(b[16:24]) / 1_000_000)
}

// gcSeed has the C agent write the workload into `tiers` tiers: the four charts for gcEphemeral seconds, a stop, then
// a start on its own cache and gc.c0 alone to the end (the other charts archived). Its cache and the window.
func gcSeed(t *testing.T, tiers int) (string, int64, int64) {
	t.Helper()
	end := time.Now().Unix()
	start := end - gcSpan + 1
	seed := ""
	for i, phase := range []struct {
		charts   int
		from, to int64
	}{{4, start, start + gcEphemeral - 1}, {1, start + gcEphemeral, end}} {
		o := daemon.Options{Binary: binaries(t)[0], StorageTiers: tiers, TierRetentionMB: [3]int{25, 25, 25}, PulseOff: true,
			SeedCache: seed, RunDir: runDir(t, Role(fmt.Sprintf("gc-seed-%d", i))), Identity: &parentIdentity}
		d, err := daemon.Start(o)
		if err != nil {
			t.Fatalf("gc seed %d: %v", i, err)
		}
		t.Cleanup(func() { _ = d.Stop() })
		if err := gcgen(phase.charts).stream(d, phase.from, phase.to); err != nil {
			t.Fatalf("gc seed %d: streaming: %v", i, err)
		}
		waitChartsLast(t, d, gcchild.Hostname, "gc.", phase.charts, phase.to, 10*time.Minute)
		if err := d.Stop(); err != nil {
			t.Fatalf("gc seed %d: stop: %v", i, err)
		}
		seed = filepath.Join(o.RunDir, "cache")
	}
	if files, _ := filepath.Glob(filepath.Join(seed, "dbengine", "datafile-1-*.ndf")); len(files) < 4 {
		t.Fatalf("gc seed: %d data files, want 4 at least", len(files))
	}
	return seed, start, end
}

// gcContexts is gcchild's contexts with their charts and dimensions, deleted ones too.
const gcContexts = "/host/gcchild/api/v1/contexts?options=full,deleted,queue"

// gcContextIDs are the contexts of gcchild a daemon lists, asked as a poll the log checks leave out (probeRe): how many
// polls each side takes is not a contract.
func gcContextIDs(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	var contexts map[string]json.RawMessage
	if err := json.Unmarshal([]byte(member(t, d, gcContexts+"&harness=wait", "contexts")), &contexts); err != nil {
		t.Fatal(err)
	}
	var ids []string
	for id := range contexts {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	return ids
}

// gcCharts are the charts gcchild's context lists, asked as a poll the log checks leave out.
func gcCharts(t *testing.T, d *daemon.Daemon, context string) []string {
	t.Helper()
	var contexts map[string]struct {
		Charts map[string]json.RawMessage `json:"charts"`
	}
	if err := json.Unmarshal([]byte(member(t, d, gcContexts+"&harness=wait", "contexts")), &contexts); err != nil {
		t.Fatal(err)
	}
	var ids []string
	for id := range contexts[context].Charts {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	return ids
}

// gcStart has C write the seed into `tiers` tiers, starts both daemons on copies with a tier-0 retention time that
// deletes file 1 at the first retention check (and dbExtra in [db]), and waits for the deletion. The window and the
// D30 victim: the gc.c0 dimension with the highest UUID, the last entry of file 2's list, whose file-1 pages C's
// walk skips there.
func gcStart(t *testing.T, tiers int, dbExtra string) (*Pair, daemon.Options, int64, int64, string) {
	t.Helper()
	seed, start, end := gcSeed(t, tiers)
	cutoff := (v2EndIn(t, seed, 1) + v2EndIn(t, seed, 2)) / 2
	opts := daemon.Options{StorageTiers: tiers, TierRetentionMB: [3]int{25, 25, 25}, SeedCache: seed, PulseOff: true,
		TierRetentionTime: [3]string{fmt.Sprintf("%ds", time.Now().Unix()-cutoff)}, DBExtra: dbExtra,
		LogsExtra: "    level = debug\n"}
	p := StartPair(t, opts, parentIdentity)
	for _, side := range p.Each() {
		if got := gcContextIDs(t, side.Daemon); strings.Join(got, " ") != "gc.x gc.y" {
			t.Fatalf("%s: gcchild's contexts at the start: %v", side.Role, got)
		}
	}
	var victims [2]string
	for i, side := range p.Each() {
		victimUUID := ""
		for uuid, name := range tierUUIDs(t, side.Daemon, gcchild.Hostname) {
			if strings.HasPrefix(name, "gc.c0.") && uuid > victimUUID {
				victims[i], victimUUID = name, uuid
			}
		}
	}
	if victims[0] != victims[1] {
		t.Fatalf("gc.c0's highest UUID: oracle %s, candidate %s", victims[0], victims[1])
	}
	victim := victims[0]
	for _, side := range p.Each() {
		waitRecord(t, side.Daemon, "deleted datafile-1-0000000001", 150*time.Second)
	}
	return p, opts, start, end, victim
}

// gcRetention compares every gcchild metric's retention per tier, and asserts the D30 victim's tier 0 apart: C's
// starts later than the candidate's (its walk dates it from a later file). The live retentions, and the contexts rules
// with the victim's first time masked.
func gcRetention(t *testing.T, p *Pair, victim string, start, end int64) ([2]map[string][]string, Rules) {
	t.Helper()
	var retention [2]map[string][]string
	for i, side := range p.Each() {
		retention[i] = tierRetention(t, side.Daemon, gcchild.Hostname, tierUUIDs(t, side.Daemon, gcchild.Hostname),
			start, end)
	}
	for name, tiers := range retention[0] {
		other := retention[1][name]
		if name == victim && len(tiers) > 0 && len(other) > 0 {
			tiers, other = tiers[1:], other[1:]
		}
		if fmt.Sprint(tiers) != fmt.Sprint(other) {
			t.Errorf("%s: oracle %v, candidate %v", name, tiers, other)
		}
	}
	if len(retention[0]) != len(retention[1]) {
		t.Errorf("metrics: oracle %d, candidate %d", len(retention[0]), len(retention[1]))
	}
	first := func(r map[string][]string) int64 {
		f, _ := strconv.ParseInt(strings.Split(r[victim][0], "..")[0], 10, 64)
		return f
	}
	if first(retention[0]) <= first(retention[1]) {
		t.Errorf("the victim %s on tier 0: oracle %s, candidate %s; want C's later (its walk)", victim,
			retention[0][victim][0], retention[1][victim][0])
	}
	// the mask below hides that dimension name in every chart; the others are compared here
	var firsts [2]map[string]int64
	for i, side := range p.Each() {
		firsts[i] = gcDimFirstTimes(t, side.Daemon)
	}
	for name, first := range firsts[0] {
		if name != victim && first != firsts[1][name] {
			t.Errorf("%s first_time_t: oracle %d, candidate %d", name, first, firsts[1][name])
		}
	}
	rules := dbengineReadRules
	rules.Masks = append(rules.Masks, Mask{Pattern: "**.dimensions." + strings.TrimPrefix(victim, "gc.c0.") +
		".first_time_t", Reason: "the D30 victim, asserted apart"})
	return retention, rules
}

// gcDimFirstTimes are gcchild's dimensions' first times in its contexts ("chart.dim"), asked as a poll the log checks
// leave out.
func gcDimFirstTimes(t *testing.T, d *daemon.Daemon) map[string]int64 {
	t.Helper()
	var contexts map[string]struct {
		Charts map[string]struct {
			Dimensions map[string]struct {
				First int64 `json:"first_time_t"`
			} `json:"dimensions"`
		} `json:"charts"`
	}
	if err := json.Unmarshal([]byte(member(t, d, gcContexts+"&harness=wait", "contexts")), &contexts); err != nil {
		t.Fatal(err)
	}
	out := map[string]int64{}
	for _, ctx := range contexts {
		for chart, c := range ctx.Charts {
			for dim, v := range c.Dimensions {
				out[chart+"."+dim] = v.First
			}
		}
	}
	return out
}

// TestRetentionContextsGC (check `retention.contexts-gc`, S5 commit 7, D77): the contexts' deep pass 120 s after a
// rotation. C writes a one-tier cache where gcchild's context gc.y and gc.x's chart gc.c1 have data in tier 0's file 1
// only (gcSeed); both daemons start on copies with a retention time that deletes file 1 at the first retention check.
// Compared: the pass removing gc.y and gc.c1 (the contexts, charts and dimensions with their times, deleted ones
// listed); every metric's retention (the D30 victim apart); the metadata writer's next context cleanup scan deleting
// gc.y's rows from SQLite (the dimension, chart, label and cleanup tables); the whole daemon log; then a restart on
// each cache and C on the candidate's, where the candidate's retention must be what it was live.
func TestRetentionContextsGC(t *testing.T) {
	if os.Getenv("PARITY_LONG") == "" {
		t.Skip("PARITY_LONG unset (about 6 minutes)")
	}
	p, opts, start, end, victim := gcStart(t, 1, "")
	// the pass runs 120 s after the deletion, on the worker's next tick
	deadline := time.Now().Add(150 * time.Second)
	for _, side := range p.Each() {
		for strings.Join(gcContextIDs(t, side.Daemon), " ") != "gc.x" {
			if time.Now().After(deadline) {
				t.Fatalf("%s: gcchild's contexts after the pass: %v", side.Role, gcContextIDs(t, side.Daemon))
			}
			time.Sleep(time.Second)
		}
	}
	live, rules := gcRetention(t, p, victim, start, end)
	compareGetWith(t, p, gcContexts, rules)
	// the next context cleanup scan (about 312 s after the start) deletes gc.y's rows; the one before it found no
	// cleanup row of gcchild and logged nothing of it
	scans := time.Now().Add(240 * time.Second)
	for _, side := range p.Each() {
		for !strings.Contains(strings.Join(scanRecords(t, side.Daemon), "\n"), "Verified the contexts of host gcchild") {
			if time.Now().After(scans) {
				t.Fatalf("%s: no scan of gcchild: %q", side.Role, scanRecords(t, side.Daemon))
			}
			time.Sleep(time.Second)
		}
	}
	caches := stopBoth(t, p)
	args := []string{"--table", "dimension", "--table", "chart", "--table", "chart_label",
		"--table", "ctx_metadata_cleanup", "--mask", "ctx_metadata_cleanup.date_created"}
	var dumps [2]string
	for i := range caches {
		dumps[i] = dumpDB(t, filepath.Join(caches[i], "netdata-meta.db"), args...)
	}
	if dumps[0] != dumps[1] {
		t.Errorf("tables differ:\noracle:\n%s\ncandidate:\n%s", dumps[0], dumps[1])
	}
	compareLogFiles(t, p, "daemon.log")
	// the candidate's retention is what its files left give (a restart rebuilds it from them, C's too)
	exact := restarted(t, opts, parentIdentity, caches, gcchild.Hostname, start, end)
	for _, d := range retentionDiff(live[1], exact) {
		t.Errorf("candidate: after the pass %s, not what the files left give", d)
	}
}

// TestRetentionExtremeCardinality (check `retention.extreme-cardinality`, S5 commit 8, D75.8, D77): the gcSeed with
// three tiers, so tiers 1 and 2 keep the ephemerals' rollups, and both daemons with `keep instances = 1` and `min
// ephemerality = 0`. Once tier 0's file 1 is deleted, the deep pass post-processes gc.y, both of whose instances then
// have no tier-0 retention: the protection clears one's retention on every tier (one NOTICE) and the collection
// removes it; gc.x keeps gc.c1 (the one instance without tier 0 it keeps). Compared: the contexts, every metric's
// retention per tier (the victim's tier 0 apart), the whole daemon log with the NOTICE; then a restart on each cache
// and C on the candidate's, where the victim's tier 0 must be the candidate's live value.
func TestRetentionExtremeCardinality(t *testing.T) {
	if os.Getenv("PARITY_LONG") == "" {
		t.Skip("PARITY_LONG unset (about 4 minutes)")
	}
	p, opts, start, end, victim := gcStart(t, 3,
		"extreme cardinality keep instances = 1\nextreme cardinality min ephemerality = 0\n")
	deadline := time.Now().Add(150 * time.Second)
	for _, side := range p.Each() {
		for len(gcCharts(t, side.Daemon, "gc.y")) != 1 {
			if time.Now().After(deadline) {
				t.Fatalf("%s: gc.y's charts after the pass: %v", side.Role, gcCharts(t, side.Daemon, "gc.y"))
			}
			time.Sleep(time.Second)
		}
		waitRecord(t, side.Daemon, "EXTREME CARDINALITY PROTECTION: host 'gcchild', context 'gc.y'", 10*time.Second)
	}
	live, rules := gcRetention(t, p, victim, start, end)
	compareGetWith(t, p, gcContexts, rules)
	caches := stopBoth(t, p)
	compareLogFiles(t, p, "daemon.log")
	// after a restart the victim's tier 0 is what the files left give (the cleared metrics regain their tier-1 and
	// tier-2 retention from the journals, on both sides): the candidate's live value was that
	exact := restarted(t, opts, parentIdentity, caches, gcchild.Hostname, start, end)
	if got, want := live[1][victim][0], exact[victim][0]; got != want {
		t.Errorf("candidate: the victim %s tier 0 %s after the pass, %s from the files left", victim, got, want)
	}
}
