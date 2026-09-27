// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/json"
	"fmt"
	"math"
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

// The S4a child (D72.4): five charts of five dimensions every second for two days, each exercising one rule of the
// tiers above 0 — s4.c0 dense; s4.c1 silent for longer than a tier-2 window; s4.c2 sending empty samples over whole
// tier-1 windows and a few seconds of every 1000; s4.c3 anomalous every 13 s; s4.c4 one point, then silent for longer
// than a tier-2 window, then dense.
const (
	s4Charts, s4Dims = 5, 5
	s4Span           = 172800
	s4SilentAt       = 50000
	s4SilentLen      = 4000
	s4EmptyAt        = 80000
	s4EmptyLen       = 300
)

var s4child = stream.HostInfo{Hostname: "s4child", MachineGUID: "b6b6b6b6-1111-4111-8111-000000000004"}

type s4gen struct {
	start int64
	dims  int
}

func (g s4gen) child() childGen {
	return childGen{host: s4child, prefix: "s4.", charts: s4Charts, dims: g.dims,
		context: func(c int) string { return fmt.Sprintf("s4.ctx%d", c) },
		skips: func(c int, t int64) bool {
			off := t - g.start
			return c == 1 && off >= s4SilentAt && off < s4SilentAt+s4SilentLen ||
				c == 4 && off > 0 && off < s4SilentLen
		},
		point: func(c, d int, t int64) (string, string) {
			off := t - g.start
			if c == 2 && (off >= s4EmptyAt && off < s4EmptyAt+s4EmptyLen || off%1000 < 7) {
				return "0", stream.FlagEmpty
			}
			flags := stream.FlagNotAnomalous
			if c == 3 && t%13 == 0 {
				flags = stream.FlagAnomalous
			}
			return strconv.FormatFloat(s3value(c, d, t), 'f', -1, 64), flags
		}}
}

// s4Steady are the contexts of the charts without skipped windows. Where a chart skips windows, its tier pages either
// fill the gap with empty records or close, as the page's slot alignment decides, and C hashes a heap pointer for it
// (random from run to run, D72.10): aggregated answers over such a gap differ between two C runs, so aggregated
// queries read these charts only; per-point queries and the records read every chart.
const s4Steady = "s4.ctx0%7Cs4.ctx2%7Cs4.ctx3"

// s4windows are the tier windows the checks read: name, after, before.
func (g s4gen) windows(end int64) map[string][2]int64 {
	silent := g.start + s4SilentAt
	empty := g.start + s4EmptyAt
	return map[string][2]int64{
		"whole":  {g.start, end},
		"start":  {g.start - 3600, g.start + 7200},
		"silent": {silent - 7200, silent + s4SilentLen + 7200},
		"empty":  {empty - 1800, empty + s4EmptyLen + 1800},
		"third":  {g.start + s4Span/3, g.start + s4Span/3 + 3*3600},
		"tail":   {end - 3*3600, end},
	}
}

// compareTierWindows reads every window through both daemons on tiers 1 and 2 (and tier 0 where it is small).
func (g s4gen) compareTierWindows(t *testing.T, p *Pair, end int64) {
	host := "/host/" + s4child.Hostname
	for name, w := range g.windows(end) {
		win := fmt.Sprintf("after=%d&before=%d", w[0], w[1])
		var paths []string
		for tier := 1; tier <= 2; tier++ {
			paths = append(paths,
				fmt.Sprintf("/api/v3/data?contexts=%s&%s&points=200&tier=%d&options=debug", s4Steady, win, tier),
				fmt.Sprintf("/api/v3/data?contexts=%s&%s&points=100&tier=%d&group=max&options=debug", s4Steady, win,
					tier))
			for c := 0; c < s4Charts; c++ {
				paths = append(paths, fmt.Sprintf("/api/v1/data?chart=s4.c%d&%s&tier=%d&options=jsonwrap", c, win, tier))
			}
		}
		if name != "whole" {
			paths = append(paths, fmt.Sprintf("/api/v3/data?contexts=%s&%s&points=100&tier=0&options=debug", s4Steady,
				win))
		}
		for _, path := range paths {
			t.Run(name+" "+path, func(t *testing.T) { compareGetWith(t, p, host+path, dbengineWriteRules(true)) })
		}
	}
}

// tierUUIDs maps each dimension's UUID to "chart.dim", from the running daemon's contexts.
func tierUUIDs(t *testing.T, d *daemon.Daemon, host string) map[string]string {
	t.Helper()
	b, err := rawExchange(d.Addr, []byte("GET /host/"+host+"/api/v1/contexts?options=full HTTP/1.1\r\n\r\n"),
		20*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	var doc struct {
		Contexts map[string]struct {
			Charts map[string]struct {
				Dimensions map[string]struct {
					UUID string `json:"uuid"`
				} `json:"dimensions"`
			} `json:"charts"`
		} `json:"contexts"`
	}
	if err := json.Unmarshal(httpBody(b), &doc); err != nil {
		t.Fatalf("contexts: %v", err)
	}
	out := map[string]string{}
	for _, ctx := range doc.Contexts {
		for chart, c := range ctx.Charts {
			for dim, v := range c.Dimensions {
				out[strings.ReplaceAll(v.UUID, "-", "")] = chart + "." + dim
			}
		}
	}
	if len(out) == 0 {
		t.Fatalf("%s: no dimensions in the contexts of %s", d.Opts.Binary, host)
	}
	return out
}

// tierRecords are a cache's aggregated records by "tier chart.dim", the records with a value (finite sum) as lines
// "end sum min max count anomalies", and how many records had no value.
func tierRecords(t *testing.T, cache string, names map[string]string) (map[string][]string, int) {
	t.Helper()
	code, out := inspect(t, "--records", cache)
	if code != 0 {
		t.Fatalf("dbengine-inspect --records %s: exit %d\n%s", cache, code, out)
	}
	records, empty := map[string][]string{}, 0
	for _, line := range strings.Split(strings.TrimSpace(out), "\n") {
		f := strings.Fields(line)
		if len(f) != 8 {
			continue
		}
		name, ok := names[f[1]]
		if !ok {
			continue
		}
		bits, _ := strconv.ParseUint(f[3], 16, 32)
		if math.IsNaN(float64(math.Float32frombits(uint32(bits)))) {
			empty++
			continue
		}
		key := f[0] + " " + name
		records[key] = append(records[key], strings.Join(f[2:], " "))
	}
	return records, empty
}

// compareTierRecords compares both caches' records with a value, per tier and dimension.
func compareTierRecords(t *testing.T, caches [2]string, names [2]map[string]string) {
	t.Helper()
	var recs [2]map[string][]string
	var empty [2]int
	for i := range caches {
		recs[i], empty[i] = tierRecords(t, caches[i], names[i])
	}
	keys := map[string]bool{}
	for i := range recs {
		for k := range recs[i] {
			keys[k] = true
		}
	}
	sorted := make([]string, 0, len(keys))
	for k := range keys {
		sorted = append(sorted, k)
	}
	sort.Strings(sorted)
	total := 0
	for _, k := range sorted {
		o, c := recs[0][k], recs[1][k]
		total += len(o)
		for i := 0; i < len(o) || i < len(c); i++ {
			if i >= len(o) || i >= len(c) || o[i] != c[i] {
				at := func(r []string) string {
					if i < len(r) {
						return r[i]
					}
					return "(none)"
				}
				t.Errorf("%s: record %d of %d/%d: oracle %s, candidate %s", k, i, len(o), len(c), at(o), at(c))
				break
			}
		}
	}
	if total == 0 {
		t.Errorf("no tier records with a value")
	}
	t.Logf("tier records with a value: %d; without: oracle %d, candidate %d", total, empty[0], empty[1])
}

// tierFiles are the names of a cache's dbengine files, tier by tier.
func tierFiles(t *testing.T, cache string) string {
	t.Helper()
	var all []string
	for _, dir := range []string{"dbengine", "dbengine-tier1", "dbengine-tier2"} {
		entries, err := os.ReadDir(filepath.Join(cache, dir))
		if err != nil {
			t.Fatal(err)
		}
		for _, e := range entries {
			if strings.HasPrefix(e.Name(), "datafile-") || strings.HasPrefix(e.Name(), "journalfile-") {
				all = append(all, dir+"/"+e.Name())
			}
		}
	}
	sort.Strings(all)
	return strings.Join(all, " ")
}

// copyCaches copies both sides' caches for a subtest that writes to them.
func copyCaches(t *testing.T, caches [2]string) [2]string {
	t.Helper()
	var out [2]string
	for i, c := range caches {
		out[i] = filepath.Join(t.TempDir(), "cache")
		copyTree(t, c, out[i])
	}
	return out
}

// TestDbengineTiers streams the S4a child into two dbengine parents with three tiers (fresh caches, 25 MiB each,
// pulse off) and compares the tiers above 0: their windows while the tail is live, every record with a value after
// the stop, the files, a restart, C on both caches, and the backfill of each mode after a restart (checks
// dbengine.tiers and dbengine.backfill, D72).
func TestDbengineTiers(t *testing.T) {
	opts := daemon.Options{StorageTiers: 3, TierRetentionMB: [3]int{25, 25, 25}, PulseOff: true,
		LogsExtra: "    level = debug\n"}
	p := StartPair(t, opts, parentIdentity)
	end := time.Now().Unix() - 3600
	g := s4gen{start: end - s4Span + 1, dims: s4Dims}
	g.child().streamBoth(t, p, g.start, end)
	var names [2]map[string]string
	for i, side := range p.Each() {
		names[i] = tierUUIDs(t, side.Daemon, s4child.Hostname)
	}

	t.Run("live", func(t *testing.T) { g.compareTierWindows(t, p, end) })
	for _, side := range p.Each() {
		if err := side.Daemon.Stop(); err != nil {
			t.Fatalf("stop %s: %v", side.Role, err)
		}
	}
	caches := [2]string{filepath.Join(p.Oracle.Opts.RunDir, "cache"), filepath.Join(p.Candidate.Opts.RunDir, "cache")}

	t.Run("records", func(t *testing.T) { compareTierRecords(t, caches, names) })
	t.Run("log", func(t *testing.T) { compareLogFilesWith(t, p, writeLogMasks, "daemon.log") })
	t.Run("files", func(t *testing.T) {
		if o, c := tierFiles(t, caches[0]), tierFiles(t, caches[1]); o != c {
			t.Errorf("files:\noracle:    %s\ncandidate: %s", o, c)
		}
		for i, side := range p.Each() {
			if code, out := inspect(t, "--normalize-v2", caches[i]); code != 0 {
				t.Errorf("%s: dbengine-inspect --normalize-v2: exit %d\n%s", side.Role, code, out)
			}
		}
	})
	t.Run("restart", func(t *testing.T) {
		r := startPair(t, opts, parentIdentity, binaries(t), copyCaches(t, caches),
			[2]Role{"restart-oracle", "restart-candidate"})
		g.compareTierWindows(t, r, end)
	})
	t.Run("handback", func(t *testing.T) {
		oracle := binaries(t)[0]
		h := startPair(t, opts, parentIdentity, [2]string{oracle, oracle}, copyCaches(t, caches),
			[2]Role{"handback-oracle", "handback-candidate"})
		g.compareTierWindows(t, h, end)
	})
	// after a restart the child resumes 1,000 s later (under one tier-2 window) with a sixth dimension per chart:
	// each mode backfills, or not, what the restart lost, and full backfills the new dimensions too
	for _, mode := range []string{"new", "none", "full"} {
		t.Run("backfill-"+mode, func(t *testing.T) {
			o := opts
			o.DBExtra = "dbengine tier backfill = " + mode
			b := startPair(t, o, parentIdentity, binaries(t), copyCaches(t, caches),
				[2]Role{Role("backfill-" + mode + "-oracle"), Role("backfill-" + mode + "-candidate")})
			more := s4gen{start: g.start, dims: s4Dims + 1}
			from, to := end+1001, end+1900
			more.child().streamBoth(t, b, from, to)
			var bnames [2]map[string]string
			for i, side := range b.Each() {
				bnames[i] = tierUUIDs(t, side.Daemon, s4child.Hostname)
			}
			// the seam skips windows on every chart: per-point answers only
			host := "/host/" + s4child.Hostname
			win := fmt.Sprintf("after=%d&before=%d", end-7200, to)
			for tier := 1; tier <= 2; tier++ {
				for c := 0; c < s4Charts; c++ {
					compareGetWith(t, b, fmt.Sprintf("%s/api/v1/data?chart=s4.c%d&%s&tier=%d&options=jsonwrap", host, c,
						win, tier), dbengineWriteRules(true))
				}
			}
			for _, side := range b.Each() {
				if err := side.Daemon.Stop(); err != nil {
					t.Fatalf("stop %s: %v", side.Role, err)
				}
			}
			compareTierRecords(t, [2]string{filepath.Join(b.Oracle.Opts.RunDir, "cache"),
				filepath.Join(b.Candidate.Opts.RunDir, "cache")}, bnames)
		})
	}
}

// TestDbengineTiersSmallGrouping repeats the S4a child's first 40,000 s with tier windows of 5 and 15 s, so the tier
// pages fill data files at run time: tier 2 rotates, its runtime v2 files equal their normalized rebuilds on both
// sides, and every record with a value compares (D72.4).
func TestDbengineTiersSmallGrouping(t *testing.T) {
	opts := daemon.Options{StorageTiers: 3, TierRetentionMB: [3]int{25, 25, 25}, TierGrouping: [3]int{0, 5, 3},
		PulseOff: true, LogsExtra: "    level = debug\n"}
	p := StartPair(t, opts, parentIdentity)
	end := time.Now().Unix() - 3600
	// about 1.6 tier-2 data files: the rotation lands far from a file boundary on both sides
	g := s4gen{start: end - 40000 + 1, dims: s4Dims}
	g.child().streamBoth(t, p, g.start, end)
	var names [2]map[string]string
	for i, side := range p.Each() {
		names[i] = tierUUIDs(t, side.Daemon, s4child.Hostname)
	}
	for _, side := range p.Each() {
		if err := side.Daemon.Stop(); err != nil {
			t.Fatalf("stop %s: %v", side.Role, err)
		}
	}
	caches := [2]string{filepath.Join(p.Oracle.Opts.RunDir, "cache"), filepath.Join(p.Candidate.Opts.RunDir, "cache")}
	compareTierRecords(t, caches, names)
	// tier 2's small pages fill its first data file; how full each file gets follows page composition (D72.10),
	// so file names are not compared here
	for i, side := range p.Each() {
		v2, _ := filepath.Glob(filepath.Join(caches[i], "dbengine-tier2", "journalfile-1-*.njfv2"))
		if len(v2) == 0 {
			t.Errorf("%s: tier 2 wrote no runtime v2 file", side.Role)
		}
	}
	for i, side := range p.Each() {
		if code, out := inspect(t, "--rebuild-v2", "--normalize-v2", caches[i]); code != 0 {
			t.Errorf("%s: runtime v2 against its normalized rebuild: exit %d\n%s", side.Role, code, out)
		}
	}
}
