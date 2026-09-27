// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The S3 writer's fake child (the generator of the S3 evidence runs, D65.N6): four charts of five dimensions every
// second — s3.c0 (context s3.gap) skipping the last 3 s of every 700 s, s3.c1 (s3.single) one point then 3000 s of
// silence, s3.c2 and s3.c3 (s3.norm) dense — each value a function of chart, dimension and time.
const (
	s3Charts, s3Dims                  = 4, 5
	s3GapEvery, s3GapLen, s3SingleGap = 700, 3, 3000
	// about 1.5 data files of 524,288 bytes: one rotation on each side, the fill point far from a file boundary
	// (`evidence/2026-09-27-s3-ingest-measurement.md` in the status repository)
	s3Span = 167000
)

var s3child = stream.HostInfo{Hostname: "s3child", MachineGUID: "b6b6b6b6-1111-4111-8111-000000000003"}

type s3gen struct{ start int64 }

func s3value(c, d int, t int64) float64 {
	return float64((t/10)%1000) + float64(c)*0.5 + float64(d)*0.125
}

func s3context(c int) string {
	switch c {
	case 0:
		return "s3.gap"
	case 1:
		return "s3.single"
	}
	return "s3.norm"
}

// skips says whether chart c sends nothing at t.
func (g s3gen) skips(c int, t int64) bool {
	return c == 0 && (t-g.start)%s3GapEvery >= s3GapEvery-s3GapLen ||
		c == 1 && t > g.start && t < g.start+s3SingleGap
}

// stored is how many points [from, to] leaves in the database, every one with a value.
func (g s3gen) stored(from, to int64) int {
	n := 0
	for t := from; t <= to; t++ {
		for c := 0; c < s3Charts; c++ {
			if !g.skips(c, t) {
				n += s3Dims
			}
		}
	}
	return n
}

// childGen is a fake child's workload: its charts (prefix + "c<n>", each with dims "d0".. and a context) and, for
// each chart and second, whether it sends nothing and else each dimension's value and flags.
type childGen struct {
	host         stream.HostInfo
	prefix       string
	charts, dims int
	context      func(c int) string
	skips        func(c int, t int64) bool
	point        func(c, d int, t int64) (value, flags string)
}

// stream connects the child to d, defines its charts, sends [from, to] and disconnects.
func (g childGen) stream(d *daemon.Daemon, from, to int64) error {
	conn, err := stream.Connect(d.Addr, d.StreamKey, g.host, stream.CapsLive)
	if err != nil {
		return err
	}
	defer conn.Close()
	for c := 0; c < g.charts; c++ {
		conn.DefineChart(stream.Chart{ID: fmt.Sprintf("%sc%d", g.prefix, c), Title: g.prefix, Units: "u", Family: "f",
			Context: g.context(c)})
		for dim := 0; dim < g.dims; dim++ {
			conn.Dimension(fmt.Sprintf("d%d", dim), "absolute", 1, 1)
		}
	}
	n := 0
	for t := from; t <= to; t++ {
		for c := 0; c < g.charts; c++ {
			if g.skips(c, t) {
				continue
			}
			conn.Begin2(fmt.Sprintf("%sc%d", g.prefix, c), 1, t)
			for dim := 0; dim < g.dims; dim++ {
				v, f := g.point(c, dim, t)
				conn.Set2(fmt.Sprintf("d%d", dim), v, f)
			}
			conn.End2()
		}
		if n++; n%2000 == 0 {
			if err := conn.Flush(); err != nil {
				return err
			}
		}
	}
	return conn.Flush()
}

// streamBoth streams [from, to] into both daemons at once, then waits until each has every chart at to.
func (g childGen) streamBoth(t *testing.T, p *Pair, from, to int64) {
	t.Helper()
	var wg sync.WaitGroup
	errs := make([]error, 2)
	for i, side := range p.Each() {
		wg.Add(1)
		go func() {
			defer wg.Done()
			errs[i] = g.stream(side.Daemon, from, to)
		}()
	}
	wg.Wait()
	for i, side := range p.Each() {
		if errs[i] != nil {
			t.Fatalf("%s: streaming: %v", side.Role, errs[i])
		}
		waitChartsLast(t, side.Daemon, g.host.Hostname, g.prefix, g.charts, to, 10*time.Minute)
	}
}

// child is the S3 workload as a childGen.
func (g s3gen) child() childGen {
	return childGen{host: s3child, prefix: "s3.", charts: s3Charts, dims: s3Dims, context: s3context, skips: g.skips,
		point: func(c, d int, t int64) (string, string) {
			return strconv.FormatFloat(s3value(c, d, t), 'f', -1, 64), stream.FlagNotAnomalous
		}}
}

func (g s3gen) stream(d *daemon.Daemon, from, to int64) error { return g.child().stream(d, from, to) }

func (g s3gen) streamBoth(t *testing.T, p *Pair, from, to int64) {
	t.Helper()
	g.child().streamBoth(t, p, from, to)
}

// waitChartsLast waits until the host has n charts with the prefix, each with its last entry at last (WaitRetention
// asks for every point, which is too much here).
func waitChartsLast(t *testing.T, d *daemon.Daemon, host, prefix string, n int, last int64, timeout time.Duration) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for {
		ok := func() bool {
			// a connection of its own per poll, tagged, which the log checks leave out (probeRe)
			req := "GET /host/" + host + "/api/v1/charts?harness=wait HTTP/1.1\r\nConnection: close\r\n\r\n"
			b, err := rawExchange(d.Addr, []byte(req), 10*time.Second)
			if err != nil {
				return false
			}
			var doc struct {
				Charts map[string]struct {
					LastEntry int64 `json:"last_entry"`
				} `json:"charts"`
			}
			if json.Unmarshal(httpBody(b), &doc) != nil {
				return false
			}
			found := 0
			for id, c := range doc.Charts {
				if strings.HasPrefix(id, prefix) {
					if c.LastEntry != last {
						return false
					}
					found++
				}
			}
			return found == n
		}()
		if ok {
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("%s: the charts %s* never reached %d", d.Opts.Binary, prefix, last)
		}
		time.Sleep(500 * time.Millisecond)
	}
}

// dbengineWriteRules add the context hash (the contexts' event counter) to the read rules; readCounts also masks the
// points read, which C's gapped and single-point pages make depend on their composition (D65.11).
func dbengineWriteRules(readCounts bool) Rules {
	r := dbengineReadRules
	r.Masks = append(append([]Mask{}, dbengineReadRules.Masks...),
		Mask{Pattern: "**.contexts_hard_hash", Reason: "context events"},
		// fresh caches: each side makes up its own chart and dimension UUIDs (their reuse is the append check's)
		Mask{Pattern: "**.uuid", Reason: "new UUIDs on each side"})
	if readCounts {
		r.Masks = append(r.Masks,
			Mask{Pattern: "db.per_tier.[].points", Reason: "points read: page composition (D65.11)"},
			Mask{Pattern: "db_points_per_tier", Reason: "points read: page composition (D65.11)"})
	}
	return r
}

// writeLogMasks hide what follows page composition, which flush timing and page alignment decide (the C-vs-C
// evidence differs there too): sizes and counts of the files written.
var writeLogMasks = []logMask{
	{regexp.MustCompile(`(indexing journalfile-1-\d{10}\.njfv2: extents )\d+(, metrics )\d+(, pages )\d+`), "${1}N${2}N${3}N"},
	{regexp.MustCompile(`(migrated journalfile-1-\d{10}\.njfv2, )[^"]+`), "${1}<size>"},
	{regexp.MustCompile(`\(size:\d+\)`), "(size:N)"},
	{regexp.MustCompile(`(loaded, size: )[0-9.]+ MiB, metrics: [0-9.]+ k`), "${1}X MiB, metrics: Y k"},
	{regexp.MustCompile(`(populated, size: )[0-9.]+ MiB, metrics: [0-9.]+ k`), "${1}X MiB, metrics: Y k"},
}

// s3windows are the data windows the checks read: name, after, before.
func (g s3gen) windows(end int64) map[string][2]int64 {
	third := g.start + s3Span/3
	skip := g.start + 100*s3GapEvery + s3GapEvery - s3GapLen
	return map[string][2]int64{
		"whole":      {g.start, end},
		"single":     {g.start - 60, g.start + s3SingleGap + 100},
		"gap":        {skip - 20, skip + 20},
		"third":      {third, third + 1200},
		"two-thirds": {third + s3Span/3, third + s3Span/3 + 1200},
		"hot":        {end - 900, end},
		"outside":    {g.start - 3600, g.start - 60},
	}
}

// compareWindows reads the windows (the hot one only with hot) through both daemons.
func (g s3gen) compareWindows(t *testing.T, p *Pair, end int64, hot bool) {
	host := "/host/" + s3child.Hostname
	for name, w := range g.windows(end) {
		if name == "hot" && !hot {
			continue
		}
		win := fmt.Sprintf("after=%d&before=%d", w[0], w[1])
		type query struct {
			path   string
			masked bool
		}
		var qs []query
		switch name {
		case "whole":
			qs = []query{
				{"/api/v3/data?contexts=s3.*&" + win + "&points=500&options=debug", true},
				{"/api/v3/data?contexts=s3.norm&" + win + "&points=500&tier=0&options=debug", false},
				{"/api/v1/data?context=s3.norm&" + win + "&points=300&group=sum&options=jsonwrap", false},
			}
		case "single":
			qs = []query{
				{"/api/v1/data?chart=s3.c1&" + win + "&tier=0&options=jsonwrap", true},
				{"/api/v3/data?contexts=s3.single&" + win + "&points=60&options=debug", true},
			}
		case "gap":
			qs = []query{{"/api/v1/data?chart=s3.c0&" + win + "&options=jsonwrap,debug", true}}
		case "third", "two-thirds":
			qs = []query{
				{"/api/v1/data?chart=s3.c2&" + win + "&tier=0&options=jsonwrap", false},
				{"/api/v3/data?contexts=s3.norm&" + win + "&points=100&options=debug", false},
			}
		case "hot":
			for c := 0; c < s3Charts; c++ {
				qs = append(qs, query{fmt.Sprintf("/api/v1/data?chart=s3.c%d&%s&options=jsonwrap", c, win), c < 2})
			}
			qs = append(qs, query{"/api/v3/data?contexts=s3.*&" + win + "&points=90&tier=0&options=debug", true})
		case "outside":
			qs = []query{{"/api/v3/data?contexts=s3.*&" + win + "&points=10", true}}
		}
		for _, q := range qs {
			t.Run(name+" "+q.path, func(t *testing.T) { compareGetWith(t, p, host+q.path, dbengineWriteRules(q.masked)) })
		}
	}
}

// cacheFiles are the names of a cache's tier-0 dbengine files.
func cacheFiles(t *testing.T, cache string) []string {
	t.Helper()
	entries, err := os.ReadDir(filepath.Join(cache, "dbengine"))
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, e := range entries {
		if strings.HasPrefix(e.Name(), "datafile-") || strings.HasPrefix(e.Name(), "journalfile-") {
			names = append(names, e.Name())
		}
	}
	sort.Strings(names)
	return names
}

// inspect runs dbengine-inspect; its exit status and output.
func inspect(t *testing.T, args ...string) (int, string) {
	t.Helper()
	out, err := exec.Command(dbengineInspect(t), args...).CombinedOutput()
	if exitErr, ok := err.(*exec.ExitError); ok {
		return exitErr.ExitCode(), string(out)
	}
	if err != nil {
		t.Fatalf("dbengine-inspect %v: %v", args, err)
	}
	return 0, string(out)
}

// TestDbengineWrite streams the S3 child into two dbengine parents (fresh caches, one tier of 25 MiB, pulse off) and
// compares what they answer while its last pages are hot, what they log, the files they leave, and how each cache
// reads after a restart, through C (hand-back), when C's cache is appended to, and after C rebuilds the Rust v2
// files (the dbengine.write, dbengine.write-handback and dbengine.v2-rebuild checks; D70).
func TestDbengineWrite(t *testing.T) {
	opts := daemon.Options{StorageTiers: 1, TierRetentionMB: [3]int{25}, PulseOff: true,
		LogsExtra: "    level = debug\n"}
	p := StartPair(t, opts, parentIdentity)
	// an hour in the past: every point is before now, and each restart reuses the last file (a day's rule)
	end := time.Now().Unix() - 3600
	g := s3gen{start: end - s3Span + 1}
	g.streamBoth(t, p, g.start, end)

	t.Run("contexts", func(t *testing.T) {
		rules := dbengineWriteRules(false)
		rules.Settle = 15 * time.Second
		compareGetWith(t, p, "/host/s3child/api/v1/contexts?options=full", rules)
		compareChartsRaw(t, p, "/host/s3child/api/v1/charts")
	})
	t.Run("data", func(t *testing.T) { g.compareWindows(t, p, end, true) })
	for _, side := range p.Each() {
		if err := side.Daemon.Stop(); err != nil {
			t.Fatalf("stop %s: %v", side.Role, err)
		}
	}
	caches := [2]string{filepath.Join(p.Oracle.Opts.RunDir, "cache"), filepath.Join(p.Candidate.Opts.RunDir, "cache")}

	t.Run("records", func(t *testing.T) { compareLogFilesWith(t, p, writeLogMasks, "daemon.log") })
	t.Run("files", func(t *testing.T) {
		if o, c := cacheFiles(t, caches[0]), cacheFiles(t, caches[1]); strings.Join(o, " ") != strings.Join(c, " ") {
			t.Errorf("files: oracle %v, candidate %v", o, c)
		}
		want := g.stored(g.start, end)
		for i, side := range p.Each() {
			if code, out := inspect(t, "--normalize-v2", caches[i]); code != 0 {
				t.Errorf("%s: dbengine-inspect --normalize-v2: exit %d\n%s", side.Role, code, out)
			}
			_, out := inspect(t, "--totals", caches[i])
			var totals map[string]struct {
				Slots      int `json:"slots"`
				EmptySlots int `json:"empty_slots"`
				Metrics    int `json:"metrics"`
			}
			if err := json.Unmarshal([]byte(out), &totals); err != nil {
				t.Fatalf("%s: totals: %v: %s", side.Role, err, out)
			}
			if got := totals["0"]; got.Slots-got.EmptySlots != want || got.Metrics != s3Charts*s3Dims {
				t.Errorf("%s: stored %d points of %d metrics, want %d of %d", side.Role,
					got.Slots-got.EmptySlots, got.Metrics, want, s3Charts*s3Dims)
			}
		}
		// both compress their extents with the same algorithms (how many extents each is page composition)
		var algorithms [2]string
		for i, side := range p.Each() {
			// one report per tier directory
			_, out := inspect(t, "--json", caches[i])
			used := map[string]bool{}
			for dec := json.NewDecoder(strings.NewReader(out)); dec.More(); {
				var report struct {
					Files []struct {
						Compression map[string]int `json:"compression"`
					} `json:"files"`
				}
				if err := dec.Decode(&report); err != nil {
					t.Fatalf("%s: dbengine-inspect --json: %v", side.Role, err)
				}
				for _, f := range report.Files {
					for algorithm := range f.Compression {
						used[algorithm] = true
					}
				}
			}
			keys := make([]string, 0, len(used))
			for k := range used {
				keys = append(keys, k)
			}
			sort.Strings(keys)
			algorithms[i] = strings.Join(keys, ",")
			// the Rust agent pads its extents with zeros (C leaves what the buffer held)
			if i == 1 && strings.Contains(out, `"extent_pad_zero": false`) {
				t.Errorf("candidate: an extent's padding is not zero")
			}
		}
		if algorithms[0] != algorithms[1] || algorithms[0] == "" {
			t.Errorf("extent compression algorithms: oracle %q, candidate %q", algorithms[0], algorithms[1])
		}
	})
	t.Run("restart", func(t *testing.T) {
		r := startPair(t, opts, parentIdentity, binaries(t), caches, [2]Role{"restart-oracle", "restart-candidate"})
		rules := dbengineWriteRules(false)
		rules.Settle = 15 * time.Second
		compareGetWith(t, r, "/host/s3child/api/v1/contexts?options=full", rules)
		g.compareWindows(t, r, end, false)
	})
	t.Run("handback", func(t *testing.T) {
		oracle := binaries(t)[0]
		h := startPair(t, opts, parentIdentity, [2]string{oracle, oracle}, caches,
			[2]Role{"handback-oracle", "handback-candidate"})
		g.compareWindows(t, h, end, false)
		for _, side := range h.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		if o, c := engineRecords(t, h.Oracle, writeLogMasks), engineRecords(t, h.Candidate, writeLogMasks); o != c {
			t.Errorf("engine records differ\n%s", firstDifference([]byte(o), []byte(c)))
		}
	})
	t.Run("append", func(t *testing.T) {
		a := startPair(t, opts, parentIdentity, binaries(t), [2]string{caches[0], caches[0]},
			[2]Role{"append-oracle", "append-candidate"})
		g.streamBoth(t, a, end+1, end+900)
		host := "/host/" + s3child.Hostname
		for _, w := range [][2]int64{{end - 600, end + 900}, {g.start, end + 900}} {
			win := fmt.Sprintf("after=%d&before=%d", w[0], w[1])
			compareGetWith(t, a, host+"/api/v3/data?contexts=s3.norm&"+win+"&points=150&options=debug",
				dbengineWriteRules(false))
			compareGetWith(t, a, host+"/api/v1/data?chart=s3.c3&"+win+"&points=150&options=jsonwrap",
				dbengineWriteRules(false))
		}
		for _, side := range a.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		compareLogFiles(t, a, "daemon.log")
		// the child's dimensions keep their rows: no new UUIDs
		dims := func(cache string) string {
			return dumpDB(t, filepath.Join(cache, "netdata-meta.db"), "--table", "dimension")
		}
		before := dims(caches[0])
		for _, side := range a.Each() {
			if got := dims(filepath.Join(side.Daemon.Opts.RunDir, "cache")); got != before {
				t.Errorf("%s: the dimension rows changed:\n%s", side.Role, firstDifference([]byte(before), []byte(got)))
			}
		}
		appended := filepath.Join(a.Candidate.Opts.RunDir, "cache")
		if code, out := inspect(t, "--normalize-v2", appended); code != 0 {
			t.Errorf("candidate-appended cache: dbengine-inspect --normalize-v2: exit %d\n%s", code, out)
		}
		// C reads the file the Rust agent appended to as the one it appended to itself
		oracle := binaries(t)[0]
		c := startPair(t, opts, parentIdentity, [2]string{oracle, oracle},
			[2]string{filepath.Join(a.Oracle.Opts.RunDir, "cache"), appended}, [2]Role{"append-c1", "append-c2"})
		win := fmt.Sprintf("after=%d&before=%d", g.start, end+900)
		compareGetWith(t, c, host+"/api/v3/data?contexts=s3.*&"+win+"&points=150&options=debug", dbengineWriteRules(true))
	})
	t.Run("v2-rebuild", func(t *testing.T) {
		// C's startup rebuild of the candidate's rotated journals equals the inspector's rebuild, byte for byte
		work := filepath.Join(t.TempDir(), "cache")
		copyTree(t, caches[1], work)
		rotated, _ := filepath.Glob(filepath.Join(work, "dbengine", "journalfile-1-*.njfv2"))
		if len(rotated) == 0 {
			t.Fatal("the candidate wrote no runtime v2 file")
		}
		for _, f := range rotated {
			if err := os.Remove(f); err != nil {
				t.Fatal(err)
			}
		}
		o := opts
		o.Binary = binaries(t)[0]
		o.SeedCache = work
		o.RunDir = runDir(t, Role("v2-rebuild"))
		o.Identity = &parentIdentity
		d, err := daemon.Start(o)
		if err != nil {
			t.Fatalf("v2-rebuild: %v", err)
		}
		t.Cleanup(func() { _ = d.Stop() })
		if err := d.Stop(); err != nil {
			t.Fatalf("v2-rebuild: stop: %v", err)
		}
		rebuilt := filepath.Join(o.RunDir, "cache")
		if code, out := inspect(t, "--rebuild-v2", rebuilt); code != 0 {
			t.Errorf("C's rebuild of the candidate's journals differs from the inspector's (exit %d)\n%s", code, out)
		}
		// the candidate's runtime v2 files, and C's, equal their rebuilds once normalized (so C's and Rust's do)
		for i, side := range p.Each() {
			if code, out := inspect(t, "--rebuild-v2", "--normalize-v2", caches[i]); code != 0 {
				t.Errorf("%s: runtime v2 against its normalized rebuild: exit %d\n%s", side.Role, code, out)
			}
		}
	})
}

// TestDbengineReceiverFallback runs ram parents whose child's own section asks for dbengine without enabling it: C
// then starts no dbengine, logs its ERR on the web thread and gives the child the [db] mode, and so does the
// candidate (N7, D68.7.5). The whole daemon.log compares, the ERR's stale errno removed.
func TestDbengineReceiverFallback(t *testing.T) {
	p := StartPair(t, daemon.Options{DBMode: "ram", StreamMemoryMode: "ram", StorageTiers: 1,
		LogsExtra: "    level = debug\n", StreamExtra: "\n[" + childHost.MachineGUID + "]\n    db = dbengine\n"},
		parentIdentity)
	for _, side := range p.Each() {
		logWorkload(t, side.Daemon)
	}
	for _, side := range p.Each() {
		if mode := member(t, side.Daemon, "/host/"+childHost.Hostname+"/api/v1/charts", "memory_mode"); mode != `"ram"` {
			t.Errorf("%s: the child's memory mode is %s, want ram", side.Role, mode)
		}
	}
	for _, side := range p.Each() {
		if err := side.Daemon.Stop(); err != nil {
			t.Fatalf("stop %s: %v", side.Role, err)
		}
	}
	compareLogFiles(t, p, "daemon.log")
	if !slicesContain(logLines(t, p.Candidate.Opts.RunDir, "daemon.log"), "dbengine is not enabled, falling back to default.") {
		t.Errorf("candidate: no fallback ERR")
	}
}

// slicesContain says whether a line holds the text.
func slicesContain(lines []string, text string) bool {
	for _, l := range lines {
		if strings.Contains(l, text) {
			return true
		}
	}
	return false
}
