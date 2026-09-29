// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The proxy's identities: the proxy itself (its stream key admits the child), the child it proxies, and the key it
// sends upstream with.
var (
	proxyIdentity = daemon.Identity{Hostname: "parity-proxy", StreamKey: "5a1e0000-0000-4000-8000-0000000000d2",
		MachineGUID: "5a1e0000-0000-4000-8000-0000000000d1"}
	proxiedHost = stream.HostInfo{Hostname: "parity-proxied", MachineGUID: "5a1e0000-0000-4000-8000-0000000000d3"}
)

const (
	proxyUpKey = "5a1e0000-0000-4000-8000-0000000000d4"
	// the child's claim id, and the parent's claim id and node id it sends down
	proxyClaimID     = "5a1e0000-0000-4000-8000-0000000000e1"
	proxyParentClaim = "5a1e0000-0000-4000-8000-0000000000e2"
	proxyNodeID      = "5a1e0000-0000-4000-8000-0000000000e3"
)

// A chart the fake child defines through the proxy, and its dimensions.
type proxyChart struct {
	id   string
	dims []proxyDim
}

type proxyDim struct{ id, algorithm, options string }

// proxyCharts are the plain profile's charts: an int gauge, an incremental counter and a float gauge.
var proxyCharts = []proxyChart{
	{"proxy.gauge", []proxyDim{{"g1", "absolute", ""}, {"g2", "absolute", ""}}},
	{"proxy.incr", []proxyDim{{"i1", "incremental", ""}}},
	{"proxy.float", []proxyDim{{"f1", "absolute", "type=float"}}},
}

// proxyFlush is defined last: its definition flushes the proxy's pending batch.
var proxyFlush = proxyChart{"proxy.flush", []proxyDim{{"x", "absolute", ""}}}

// proxyVariant is one row of TestProxyTranscript.
type proxyVariant struct {
	caps uint32 // the child's
	// refused are the capabilities the stub drops from the proxy's offer, beyond compression
	refused uint32
	// the child's number encodings, whether it sends slots (charts from 7), and its hops
	ints, doubles stream.Encoding
	slots         bool
	hops          int
	// section is appended to the child's [<guid>] section of the proxy's stream.conf
	section string
	// extra charts the child defines beside proxyCharts
	extra []proxyChart
	// ticks is how many seconds of data the body sends, each followed by its marker
	ticks int
	// function, when positive, is the tick after which the child registers a host function
	function int
	// malformedAt, when positive, is the tick whose lines are malformed (malformedTick) instead
	malformedAt int
	// metadata sends the child's claim id and variables after tick 2, and the parent's NODE_ID down after tick 4
	metadata bool
	// sections replaces the run: four children resolve their proxy settings from their own sections, their keys'
	// and [stream] (proxySectionsRun)
	sections bool
	// v1in makes the child a v1 one (BEGIN/SET/END, no replication), collecting once a second: the proxy runs each
	// collection through its own clock, so its blocks compare by shape, their times and values masked
	v1in bool
	// records, when set, replaces the ticks: the child writes these lines after its charts' replication, and the
	// proxies' PLUGINSD records compare instead of the transcript
	records func(c *stream.Conn, base int64)
	// gated variants run against the candidate only with PARITY_PROXY=1 (8f: v1in and metadata, which need 8h's
	// relays: the path again when the retention changes, CLAIMED_ID up and NODE_ID down)
	gated bool
}

// The child profiles (design §2.2): plain (decimal, no slots) and C-like (IEEE754 and slots in base64, two hops).
const (
	proxyPlainCaps = stream.CapsReplication | stream.CapFloatBaseline | stream.CapFunctions
	proxyCLikeCaps = proxyPlainCaps | stream.CapIEEE754 | stream.CapSlots
)

var proxyVariants = map[string]proxyVariant{
	// the proxy copies the child's words when both sides speak alike
	"copy": {caps: proxyPlainCaps, refused: stream.CapIEEE754, ticks: 12},
	// the parent takes IEEE754 the child does not send: every number re-encoded in base64
	"reencode-up": {caps: proxyPlainCaps, ticks: 12},
	// the child sends IEEE754 and slots the parent refuses: hex and decimal, the proxy's own slots, one more hop
	"reencode-down": {caps: proxyCLikeCaps, refused: stream.CapIEEE754, ints: stream.EncBase64,
		doubles: stream.EncBase64, slots: true, hops: 2, ticks: 12},
	// a child without float baselines, a parent that takes IEEE754: the float dimension's value as a double
	"float-up": {caps: proxyPlainCaps &^ stream.CapFloatBaseline, ticks: 12},
	// a parent without IEEE754 nor float baselines: the float dimension as a truncated integer
	"float-down": {caps: proxyPlainCaps, refused: stream.CapIEEE754 | stream.CapFloatBaseline, ticks: 12},
	// batches close at 101 blocks or 10,836 bytes (an 80-dimension chart), and a function's registration flushes
	// the held blocks at the next gate (tick 42, between two of the fifth-tick batches)
	"batch": {caps: proxyPlainCaps, refused: stream.CapIEEE754, ticks: 60, function: 42,
		extra: []proxyChart{proxyWide}},
	// a parent without INTERPOLATED nor REPLICATION gets v1 from the proxy: C writes the collected values it never
	// set, zeros (D106.3), still batched
	"v1up": {caps: proxyPlainCaps, refused: stream.CapIEEE754 | stream.CapInterpolated | stream.CapReplication,
		ticks: 12},
	// a BEGIN2 after a BEGIN2 without END2, of the same chart and of another: the parser unlocks the stale collection
	// lock and says so (the records; D106.4, commit 8d)
	"malformed-records": {caps: proxyPlainCaps, refused: stream.CapIEEE754, records: malformedLines},
	// a child's proxy settings come from its [<guid>] section, else its key's (a repeated [<key>] header merging),
	// else [stream]: G1 by its section to A, G2 by its key to B, G3 by its section's switch and [stream] to C, G4
	// (proxying off) nowhere
	"sections": {caps: proxyPlainCaps, sections: true},
	// a v1 child: the proxy commits one block per chart per collection, and the 101st commit closes the first batch
	"v1in": {caps: stream.CapsLiveV1, refused: stream.CapIEEE754, ticks: 34, v1in: true, gated: true},
	// the child's metadata overtakes the batch (CLAIMED_ID, VARIABLE HOST) or rides the chart's next block (VARIABLE
	// CHART); the parent's NODE_ID comes down to the child
	"metadata": {caps: proxyPlainCaps | stream.CapClaim | stream.CapNodeID | stream.CapPaths, refused: stream.CapIEEE754,
		ticks: 8, metadata: true, gated: true},
	// the same through the proxy: what it forwards of a BEGIN2 without END2 (C closes the block before the next BEGIN2
	// and forwards the other chart's under the first's gate)
	"malformed": {caps: proxyPlainCaps, refused: stream.CapIEEE754, ticks: 6, malformedAt: 3},
	// a chart the proxy's pattern excludes is never defined upstream and makes no commits
	"pattern": {caps: proxyPlainCaps, refused: stream.CapIEEE754, ticks: 12,
		section: "    proxy send charts matching = !proxy.excluded *\n",
		extra:   []proxyChart{{"proxy.excluded", []proxyDim{{"e1", "absolute", ""}}}}},
}

// proxyWide is a chart of 80 dimensions: a few of its blocks pass the 10,836 bytes a batch holds.
var proxyWide = func() proxyChart {
	ch := proxyChart{id: "proxy.wide"}
	for d := range 80 {
		ch.dims = append(ch.dims, proxyDim{id: fmt.Sprintf("w%02d", d), algorithm: "absolute"})
	}
	return ch
}()

// replicates tells whether the child offers replication (it defines its charts' retention and answers requests).
func (v proxyVariant) replicates() bool { return v.caps&stream.CapReplication != 0 }

// charts are the variant's charts in the child's order: proxyCharts, then its extra ones.
func (v proxyVariant) charts() []proxyChart {
	return append(slices.Clone(proxyCharts), v.extra...)
}

// slot is a chart's or a dimension's slot word in the child's encoding, none without slots.
func (v proxyVariant) slot(n int) string {
	if !v.slots {
		return ""
	}
	return stream.EncodeU64(v.ints, uint64(n))
}

// proxyTick is one second of data at base+2+k in the variant's encodings: explicit values, `#` for a value the
// proxy computes, flags RA and anomalous, and a gap every fifth tick.
func proxyTick(c *stream.Conn, v proxyVariant, base int64, k int) {
	ts := stream.EncodeI64(v.ints, base+2+int64(k))
	one := stream.EncodeI64(v.ints, 1)
	n := func(x int) string { return stream.EncodeI64(v.ints, int64(x)) }
	for ci, ch := range v.charts() {
		c.Begin2Raw(v.slot(7+ci), ch.id, one, ts, "#")
		for di, d := range ch.dims {
			slot := v.slot(1 + di)
			switch {
			case d.options == "type=float" && k%5 == 0:
				c.Set2Raw(slot, d.id, n(0), "NAN", stream.FlagEmpty)
			case d.options == "type=float":
				c.Set2Raw(slot, d.id, n(k), stream.EncodeF64(v.doubles, float64(k)+0.25), stream.FlagNotAnomalous)
			case d.algorithm == "incremental":
				c.Set2Raw(slot, d.id, n(1000+10*k), stream.EncodeF64(v.doubles, 10), stream.FlagAnomalous)
			case di == 1:
				c.Set2Raw(slot, d.id, n(2*k), "#", "RA")
			default:
				c.Set2Raw(slot, d.id, n(k), stream.EncodeF64(v.doubles, float64(k)), stream.FlagNotAnomalous)
			}
		}
		c.End2()
	}
}

// proxySide is one implementation's proxy between the fake child and the recording stub.
type proxySide struct {
	stub  *stream.Parent
	proxy *daemon.Daemon
	child *stream.Conn
}

// TestProxyTranscript (check `stream.proxy-transcript`, milestone 7 commit 8c, D106.12-13, D114.8, D115; design
// `evidence/2026-09-29-design-m7-commit8c.md`): the same fake child's bytes go, in lockstep, through a C proxy and a
// Rust proxy (`[stream] enabled = no`; the child's `[<guid>]` section proxies it to a recording stub), at fixed
// timestamps an hour old, so replication windows and stored points are deterministic. Compared: what each stub
// received (the request, the definitions, the replication answers, the blocks, each tick's marker), wall clocks
// masked. A `VARIABLE HOST` marker after each tick counts, by its position, the blocks the proxy committed before it
// (C commits it out of band). Variants for the candidate are gated (PARITY_PROXY=1) until commit 8's parts pass them.
func TestProxyTranscript(t *testing.T) {
	bins := binaries(t)
	oracleOnly := bins[0] == bins[1]
	for _, name := range slices.Sorted(maps.Keys(proxyVariants)) {
		v := proxyVariants[name]
		t.Run(name, func(t *testing.T) {
			if v.gated && !oracleOnly && os.Getenv("PARITY_PROXY") != "1" {
				t.Skip("gated until the Rust proxy passes it (PARITY_PROXY=1 runs it)")
			}
			if v.sections {
				got := proxySectionsRun(t, name, bins)
				if want := []string{"G1 -> A", "G2 -> B", "G3 -> C"}; !slices.Equal(got[0], want) {
					t.Errorf("the oracle's children went %v, want %v", got[0], want)
				}
				diffLines(t, "where the children went", got[0], got[1])
				return
			}
			if v.records != nil {
				got := proxyRecordsRun(t, name, v, bins)
				if !strings.Contains(strings.Join(got[0], "\n"), "stale data collection lock") {
					t.Errorf("the oracle logged no stale lock: %v", got[0])
				}
				diffLines(t, "the proxies' parser records", got[0], got[1])
				t.Logf("records:\n%s", strings.Join(got[0], "\n"))
				return
			}
			got := proxyRun(t, name, v, bins)
			diffLines(t, "the stubs' transcripts", got[0], got[1])
			t.Logf("transcript:\n%s", strings.Join(got[0], "\n"))
		})
	}
}

// proxyRun drives both sides through the phases and returns each stub's normalized transcript.
func proxyRun(t *testing.T, name string, v proxyVariant, bins [2]string) [2][]string {
	t.Helper()
	base := time.Now().Unix()/60*60 - 3600
	charts := append(v.charts(), proxyFlush)
	var sides [2]*proxySide
	for i, role := range []Role{"proxy-oracle", "proxy-candidate"} {
		sides[i] = startProxySide(t, Role(string(role)+"-"+name), bins[i], v, base)
	}
	all := func(f func(c *stream.Conn)) {
		for _, s := range sides {
			if err := s.child.Burst(func() { f(s.child) }); err != nil {
				t.Fatalf("child write: %v", err)
			}
		}
	}
	ids := func(cs []proxyChart) []string {
		var out []string
		for _, c := range cs {
			out = append(out, c.id)
		}
		return out
	}
	// P1: the charts defined and replicated from the child
	all(func(c *stream.Conn) {
		for ci, ch := range v.charts() {
			defineProxyChart(c, v, 7+ci, ch, base)
		}
	})
	for i, s := range sides {
		if !v.replicates() {
			break
		}
		if err := s.child.WaitGranted(ids(v.charts()), 30*time.Second); err != nil {
			t.Fatalf("side %d: %v", i, err)
		}
	}
	// P2: the first collection starts the proxied host's sender
	all(func(c *stream.Conn) { pokeProxyCharts(c, v, v.charts(), base+1, true) })
	sessions := [2]*stream.Session{}
	// with PARITY_KEEP=1 each proxy's run directory keeps what its stub received, a failed run's too
	t.Cleanup(func() {
		for i, s := range sessions {
			if s != nil && os.Getenv("PARITY_KEEP") == "1" {
				_ = os.WriteFile(filepath.Join(sides[i].proxy.Opts.RunDir, "upstream.txt"), s.Data(), 0o644)
			}
		}
	})
	for i, s := range sides {
		if sessions[i] = s.stub.WaitSession(1, 30*time.Second); sessions[i] == nil {
			t.Fatalf("side %d: the proxy never connected upstream", i)
		}
		if !sessions[i].WaitData(func(b []byte) bool { return strings.Contains(string(b), "\nOVERWRITE labels\n") },
			30*time.Second) {
			t.Fatalf("side %d: no host labels upstream", i)
		}
	}
	// P3: the definitions go up, and the stub's plan replicates each chart in three windows
	all(func(c *stream.Conn) { pokeProxyCharts(c, v, v.charts(), base+2, false) })
	for i := range sides {
		// without replication upstream the definitions are enough
		done := func(b []byte) bool { return rendTrueCount(b) >= len(proxyCharts) }
		if v.refused&stream.CapReplication != 0 {
			done = func(b []byte) bool { return len(proxyChartLineRe.FindAll(b, -1)) >= len(proxyCharts) }
		}
		if !sessions[i].WaitData(done, 30*time.Second) {
			t.Fatalf("side %d: the definitions or their replication upstream never finished: %s", i, sessions[i].Data())
		}
	}
	time.Sleep(time.Second)
	// P4: the variant's ticks, each followed by its marker
	for k := 1; k <= v.ticks; k++ {
		all(func(c *stream.Conn) {
			switch {
			case v.v1in:
				pokeProxyCharts(c, v, v.charts(), 0, false)
			case k == v.malformedAt:
				malformedTick(c, base, k)
			default:
				proxyTick(c, v, base, k)
			}
			c.Variable("HOST", "proxy_marker", strconv.Itoa(k))
			if k == v.function {
				c.FunctionGlobal("proxy-fn", "a proxied function")
			}
			if v.metadata && k == 2 {
				c.ClaimedID(proxiedHost.MachineGUID, proxyClaimID)
				c.Variable("CHART", "proxy_cvar", "5")
				c.Variable("HOST", "proxy_hvar", "7")
				c.Variable("HOST", "proxy_hvar", "7")
			}
		})
		if v.metadata && k == 4 {
			for i := range sessions {
				if err := sessions[i].Send(fmt.Sprintf("NODE_ID '%s' '%s' 'https://nodeid.invalid'", proxyParentClaim,
					proxyNodeID)); err != nil {
					t.Fatalf("side %d: NODE_ID: %v", i, err)
				}
			}
		}
		if v.v1in {
			time.Sleep(time.Second)
		} else {
			time.Sleep(50 * time.Millisecond)
		}
	}
	// P5: a definition flushes the pending batch
	flushSlot := 7 + len(v.charts())
	all(func(c *stream.Conn) { defineProxyChart(c, v, flushSlot, proxyFlush, base) })
	for i, s := range sides {
		if !v.replicates() {
			break
		}
		if err := s.child.WaitGranted(ids(charts), 30*time.Second); err != nil {
			t.Fatalf("side %d: %v", i, err)
		}
	}
	all(func(c *stream.Conn) { pokeProxyCharts(c, v, []proxyChart{proxyFlush}, base+2+int64(v.ticks)+1, true) })
	for i := range sides {
		if !sessions[i].WaitData(func(b []byte) bool { return proxyFlushDefRe.Match(b) }, 30*time.Second) {
			t.Fatalf("side %d: the flush chart never went up", i)
		}
	}
	time.Sleep(2 * time.Second)
	var out [2][]string
	for i, s := range sides {
		_ = s.proxy.Stop()
		transcript := proxyTranscript(string(sessions[i].Data()))
		if v.v1in {
			for j, l := range transcript {
				if proxyClockedRe.MatchString(l) {
					transcript[j] = proxyNumberRe.ReplaceAllString(l, "N")
				}
			}
		}
		out[i] = append(requestLines(sessions[i].Request), transcript...)
		out[i] = append(append(out[i], "== child"), proxyDownstream(s.child.Downstream())...)
	}
	return out
}

// proxySectionsRun starts, on each side, three stubs (A, B, C), a proxy whose stream.conf spreads the proxy settings
// over sections, and four children; after 30 s it returns which child reached which stub.
func proxySectionsRun(t *testing.T, name string, bins [2]string) [2][]string {
	t.Helper()
	const k1, k2 = "5a1e0000-0000-4000-8000-0000000000f1", "5a1e0000-0000-4000-8000-0000000000f2"
	guids := []string{"5a1e0000-0000-4000-8000-0000000000f5", "5a1e0000-0000-4000-8000-0000000000f6",
		"5a1e0000-0000-4000-8000-0000000000f7", "5a1e0000-0000-4000-8000-0000000000f8"}
	base := time.Now().Unix()/60*60 - 3600
	var out [2][]string
	for i, role := range []Role{"proxy-oracle", "proxy-candidate"} {
		stubs := map[string]*stream.Parent{}
		for _, n := range []string{"A", "B", "C"} {
			p, err := stream.StartParent(nil)
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { p.Close() })
			stubs[n] = p
		}
		id := proxyIdentity
		d, err := daemon.Start(daemon.Options{Binary: bins[i], RunDir: runDir(t, Role(string(role)+"-"+name)),
			Identity: &id, StorageTiers: 1,
			StreamSection: fmt.Sprintf("    destination = %s\n    api key = %s\n    reconnect delay = 5\n",
				stubs["C"].Addr(), proxyUpKey),
			StreamExtra: fmt.Sprintf("\n[%[1]s]\n    enabled = yes\n    type = api\n"+
				"\n[%[1]s]\n    proxy enabled = yes\n    proxy destination = %[3]s\n    proxy api key = %[5]s\n"+
				"\n[%[2]s]\n    enabled = yes\n    type = api\n"+
				"\n[%[6]s]\n    proxy enabled = yes\n    proxy destination = %[4]s\n    proxy api key = %[5]s\n"+
				"\n[%[7]s]\n    proxy enabled = yes\n",
				k1, k2, stubs["B"].Addr(), stubs["A"].Addr(), proxyUpKey, guids[0], guids[2])})
		if err != nil {
			t.Fatalf("start %s: %v", role, err)
		}
		t.Cleanup(func() { _ = d.Stop() })
		keys := []string{d.StreamKey, k1, k2, k2}
		for g, guid := range guids {
			host := stream.HostInfo{Hostname: fmt.Sprintf("proxied-g%d", g+1), MachineGUID: guid}
			c, err := stream.Connect(d.Addr, keys[g], host, proxyPlainCaps)
			if err != nil {
				t.Fatalf("%s: child G%d: %v", role, g+1, err)
			}
			t.Cleanup(func() { _ = c.Close() })
			ch := proxyCharts[0]
			c.Serve(map[string]stream.ReplayChart{ch.id: {FirstT: base - 60, LastT: base, UpdateEvery: 1}}, base,
				func(chart string, after, before int64) []stream.ReplayRow {
					return proxyRows(proxyVariant{}, chart, after, before)
				})
			_ = c.Burst(func() { defineProxyChart(c, proxyVariant{caps: proxyPlainCaps}, 7, ch, base) })
			if err := c.WaitGranted([]string{ch.id}, 30*time.Second); err != nil {
				t.Fatalf("%s: child G%d: %v", role, g+1, err)
			}
			_ = c.Burst(func() { pokeProxyCharts(c, proxyVariant{}, []proxyChart{ch}, base+1, true) })
		}
		// the connector's first pass comes after [5, 10) s; G4 must stay unconnected past it
		time.Sleep(30 * time.Second)
		for _, n := range []string{"A", "B", "C"} {
			for _, sess := range stubs[n].Sessions() {
				g := slices.Index(guids, sess.Request.Params.Get("machine_guid"))
				out[i] = append(out[i], fmt.Sprintf("G%d -> %s", g+1, n))
			}
		}
		sort.Strings(out[i])
		// a child that reconnected is listed once
		out[i] = slices.Compact(out[i])
		_ = d.Stop()
	}
	return out
}

// malformedLines are a BEGIN2 repeated without its END2, then a BEGIN2 of another chart without the first's END2.
func malformedLines(c *stream.Conn, base int64) {
	at := func(d int64) string { return strconv.FormatInt(base+d, 10) }
	c.Begin2Raw("", "proxy.gauge", "1", at(1), "#")
	c.Set2Raw("", "g1", "1", "1", stream.FlagNotAnomalous)
	c.Begin2Raw("", "proxy.gauge", "1", at(2), "#")
	c.Set2Raw("", "g1", "2", "2", stream.FlagNotAnomalous)
	c.End2()
	c.Begin2Raw("", "proxy.gauge", "1", at(3), "#")
	c.Set2Raw("", "g1", "3", "3", stream.FlagNotAnomalous)
	c.Begin2Raw("", "proxy.incr", "1", at(3), "#")
	c.Set2Raw("", "i1", "1030", "10", stream.FlagNotAnomalous)
	c.End2()
}

// malformedTick is tick k's lines, malformed: the gauge's BEGIN2 repeated without its END2, then the counter's BEGIN2
// and the float's without the counter's END2 (plain encodings).
func malformedTick(c *stream.Conn, base int64, k int) {
	t := strconv.FormatInt(base+2+int64(k), 10)
	c.Begin2Raw("", "proxy.gauge", "1", t, "#")
	c.Set2Raw("", "g1", strconv.Itoa(k), strconv.Itoa(k), stream.FlagNotAnomalous)
	c.Begin2Raw("", "proxy.gauge", "1", t, "#")
	c.Set2Raw("", "g1", strconv.Itoa(k), strconv.Itoa(k), stream.FlagNotAnomalous)
	c.Set2Raw("", "g2", strconv.Itoa(2*k), "#", "RA")
	c.End2()
	c.Begin2Raw("", "proxy.incr", "1", t, "#")
	c.Set2Raw("", "i1", strconv.Itoa(1000+10*k), "10", stream.FlagAnomalous)
	c.Begin2Raw("", "proxy.float", "1", t, "#")
	c.Set2Raw("", "f1", strconv.Itoa(k), strconv.Itoa(k), stream.FlagNotAnomalous)
	c.End2()
}

// proxyRecordsRun defines and replicates the charts on both sides, writes the variant's lines, and returns each
// proxy's PLUGINSD records.
func proxyRecordsRun(t *testing.T, name string, v proxyVariant, bins [2]string) [2][]string {
	t.Helper()
	base := time.Now().Unix()/60*60 - 3600
	var out [2][]string
	var sides [2]*proxySide
	for i, role := range []Role{"proxy-oracle", "proxy-candidate"} {
		sides[i] = startProxySide(t, Role(string(role)+"-"+name), bins[i], v, base)
	}
	var ids []string
	for _, ch := range v.charts() {
		ids = append(ids, ch.id)
	}
	for i, s := range sides {
		if err := s.child.Burst(func() {
			for ci, ch := range v.charts() {
				defineProxyChart(s.child, v, 7+ci, ch, base)
			}
		}); err != nil {
			t.Fatal(err)
		}
		if err := s.child.WaitGranted(ids, 30*time.Second); err != nil {
			t.Fatalf("side %d: %v", i, err)
		}
		if err := s.child.Burst(func() { v.records(s.child, base) }); err != nil {
			t.Fatal(err)
		}
	}
	time.Sleep(2 * time.Second)
	for i, s := range sides {
		_ = s.proxy.Stop()
		// the receiver's records; the PLUGINSD thread's own (its shutdown) belong to plugins.d, not ported yet
		for _, r := range pluginsdRecords(t, s.proxy) {
			if strings.Contains(r, "thread=STREAM[n]") {
				out[i] = append(out[i], r)
			}
		}
	}
	return out
}

// startProxySide starts a side's stub, its proxy and the fake child connected to the proxy (serving replication).
func startProxySide(t *testing.T, role Role, bin string, v proxyVariant, base int64) *proxySide {
	t.Helper()
	plan := map[string][]stream.ReplayWindow{}
	for _, ch := range proxyCharts {
		if v.v1in {
			// the proxy stores a v1 child's points at its own clock, outside the fixed windows: start at once
			break
		}
		plan[ch.id] = []stream.ReplayWindow{{After: base - 60, Before: base - 30}, {After: base - 30, Before: base - 1},
			{Start: true, After: base - 1, Before: base + 2}}
	}
	stub, err := stream.StartParent(func(r stream.Request) stream.Answer {
		a := stream.Answer{Reply: stream.VCaps(r.Caps() &^ (stream.CapsCompression | v.refused))}
		if v.refused&stream.CapReplication == 0 {
			a.Replay = stream.ReplayPlan(plan)
		}
		return a
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { stub.Close() })
	id := proxyIdentity
	d, err := daemon.Start(daemon.Options{Binary: bin, RunDir: runDir(t, role), Identity: &id, StorageTiers: 1,
		StreamSection: "    reconnect delay = 5\n",
		StreamExtra: fmt.Sprintf("\n[%s]\n    proxy enabled = yes\n    proxy destination = %s\n    proxy api key = %s\n%s",
			proxiedHost.MachineGUID, stub.Addr(), proxyUpKey, v.section)})
	if err != nil {
		t.Fatalf("start %s: %v", role, err)
	}
	t.Cleanup(func() { _ = d.Stop() })
	host := proxiedHost
	host.Hops = v.hops
	c, err := stream.Connect(d.Addr, d.StreamKey, host, v.caps)
	if err != nil {
		t.Fatalf("%s: the child's connection: %v", role, err)
	}
	t.Cleanup(func() { _ = c.Close() })
	retention := map[string]stream.ReplayChart{}
	for ci, ch := range append(v.charts(), proxyFlush) {
		retention[ch.id] = stream.ReplayChart{FirstT: base - 60, LastT: base, UpdateEvery: 1, Slot: v.slot(7 + ci)}
	}
	c.Serve(retention, base, func(chart string, after, before int64) []stream.ReplayRow {
		return proxyRows(v, chart, after, before)
	})
	return &proxySide{stub: stub, proxy: d, child: c}
}

// proxyRows are the child's retained points: every second of the window, each dimension a small value of its time.
func proxyRows(v proxyVariant, chart string, after, before int64) []stream.ReplayRow {
	var dims []proxyDim
	for _, ch := range append(v.charts(), proxyFlush) {
		if ch.id == chart {
			dims = ch.dims
		}
	}
	var rows []stream.ReplayRow
	for ts := after + 1; ts <= before; ts++ {
		row := stream.ReplayRow{T: ts}
		for j, d := range dims {
			row.Dims = append(row.Dims, stream.ReplayValue{ID: d.id, Collected: stream.EncodeI64(v.ints, ts%97+int64(j)),
				Flags: stream.FlagNotAnomalous, Slot: v.slot(1 + j)})
		}
		rows = append(rows, row)
	}
	return rows
}

// defineProxyChart writes a chart's definition with the child's retention (base-60, base], in slot `slot` when the
// child sends slots.
func defineProxyChart(c *stream.Conn, v proxyVariant, slot int, ch proxyChart, base int64) {
	c.DefineChart(stream.Chart{ID: ch.id, Title: ch.id, Units: "u", Family: "f", Context: ch.id, Slot: v.slot(slot)})
	for j, d := range ch.dims {
		c.DimensionWith(v.slot(1+j), d.id, d.id, d.algorithm, 1, 1, d.options)
	}
	c.CLabel("proxied", "yes")
	c.CLabelCommit()
	if v.replicates() {
		c.ChartDefinitionEnd(base-60, base, base)
	}
}

// pokeProxyCharts is one collection of each chart at `at`, in the child's encodings and slots; a v1 child's is
// BEGIN with the microseconds since its last (0 on its first) and its dimensions' values.
func pokeProxyCharts(c *stream.Conn, v proxyVariant, charts []proxyChart, at int64, first bool) {
	order := append(v.charts(), proxyFlush)
	one := stream.EncodeI64(v.ints, 1)
	for _, ch := range charts {
		if v.v1in {
			usec := int64(1_000_000)
			if first {
				usec = 0
			}
			c.Begin(ch.id, usec)
			for j, d := range ch.dims {
				c.Set(d.id, strconv.Itoa(10+j))
			}
			c.End()
			continue
		}
		slot := 7 + slices.IndexFunc(order, func(o proxyChart) bool { return o.id == ch.id })
		c.Begin2Raw(v.slot(slot), ch.id, one, stream.EncodeI64(v.ints, at), "#")
		for j, d := range ch.dims {
			c.Set2Raw(v.slot(1+j), d.id, one, stream.EncodeF64(v.doubles, 1), stream.FlagNotAnomalous)
		}
		c.End2()
	}
}

var (
	rendTrueRe = regexp.MustCompile(`(?m)^REND .* true `)
	// a v1 child's lines on the proxy: the lines its clock decides (times, collected values, retention) and their
	// numbers (not a slot's or a flag's)
	proxyClockedRe   = regexp.MustCompile(`^(BEGIN2|SET2|RDSTATE|RSSTATE|REND|RBEGIN|CHART_DEFINITION_END) `)
	proxyNumberRe    = regexp.MustCompile(`\b\d+(\.\d+)?\b`)
	proxyChartLineRe = regexp.MustCompile(`(?m)^CHART `)
	proxyFlushDefRe  = regexp.MustCompile(`(?m)^CHART (?:SLOT:\S+ )?"proxy\.flush"`)
)

func rendTrueCount(b []byte) int { return len(rendTrueRe.FindAll(b, -1)) }

var (
	// a definition's end carries the proxy's own clock
	proxyDefEndRe = regexp.MustCompile(`^(CHART_DEFINITION_END \S+ \S+) \S+$`)
	// a replication step's third number and REND's last are the proxy's clock (C writes REND's verdict followed by
	// two spaces)
	proxyStepRe  = regexp.MustCompile(`^(RBEGIN (?:SLOT:\S+ )?'[^']*' \S+ \S+) \S+$`)
	proxyRendRe  = regexp.MustCompile(`^(REND .*) \S+$`)
	proxyTimesRe = regexp.MustCompile(`"(since|start_time|shutdown_time)":\d+`)
)

var (
	proxyChartRe  = regexp.MustCompile(`^CHART (?:SLOT:\S+ )?"([^"]*)"`)
	proxyReplayRe = regexp.MustCompile(`^RBEGIN (?:SLOT:\S+ )?'([^']+)'$`)
	// the lines of a definition after its CHART
	proxyDefLineRe = regexp.MustCompile(`^(CLABEL |CLABEL_COMMIT$|DIMENSION |CHART_DEFINITION_END )`)
)

// proxyTranscript is what a stub received, in comparable parts. First the lines before the first definition (the
// host labels sorted: C prints them in heap order). Then each chart's definitions and replication answers, in the
// order the chart got them: the proxy's replication threads commit the answers of different charts in any order.
// Then the stream in arrival order: the blocks, the markers, a `DEF <chart>` where each definition came, and every
// other line. Last the last stream path (the sender's side sends one when its NODE_ID or retention changes, racing
// the receiver's lines). The proxy's clocks and the path's times are masked, chart labels sorted within their run,
// FUNCTION lines left out (D100.9).
func proxyTranscript(data string) []string {
	var hooks, labels, flow, lastPath []string
	charts := map[string][]string{}
	lines := strings.Split(strings.TrimSuffix(data, "\n"), "\n")
	defined := false
	for i := 0; i < len(lines); i++ {
		l := proxyMask(lines[i])
		if m := proxyChartRe.FindStringSubmatch(l); m != nil {
			// a definition: its CHART, CLABEL and DIMENSION lines, and its CHART_DEFINITION_END (sent only with
			// REPLICATION)
			charts[m[1]] = append(charts[m[1]], l)
			for i+1 < len(lines) && proxyDefLineRe.MatchString(lines[i+1]) {
				i++
				charts[m[1]] = append(charts[m[1]], proxyMask(lines[i]))
				if strings.HasPrefix(lines[i], "CHART_DEFINITION_END ") {
					break
				}
			}
			flow = append(flow, "DEF "+m[1])
			defined = true
			continue
		}
		if strings.HasPrefix(l, "JSON ") {
			// a stream path: the sender's side commits it (a NODE_ID, retention) racing the receiver's lines, so
			// only the last one compares, out of the positions
			payload := []string{l}
			for i+1 < len(lines) && !strings.HasPrefix(lines[i], "JSON_PAYLOAD_END") {
				i++
				payload = append(payload, proxyMask(lines[i]))
			}
			lastPath = payload
			continue
		}
		if m := proxyReplayRe.FindStringSubmatch(l); m != nil {
			// a replication answer, up to its REND
			for ; i < len(lines); i++ {
				charts[m[1]] = append(charts[m[1]], proxyMask(lines[i]))
				if strings.HasPrefix(lines[i], "REND ") {
					break
				}
			}
			continue
		}
		switch {
		case strings.HasPrefix(l, "FUNCTION "):
		case !defined && strings.HasPrefix(l, "LABEL "):
			labels = append(labels, l)
		case !defined:
			hooks = append(hooks, l)
		default:
			flow = append(flow, l)
		}
	}
	sort.Strings(labels)
	out := append([]string{"== hooks"}, append(labels, hooks...)...)
	for _, id := range slices.Sorted(maps.Keys(charts)) {
		out = append(out, "== chart "+id)
		out = append(out, sortedLabelRuns(charts[id])...)
	}
	out = append(append(out, "== stream"), flow...)
	return append(append(out, "== last path"), lastPath...)
}

// sortedLabelRuns sorts each run of CLABEL lines: C prints a chart's labels in heap-address order (RECIPES).
func sortedLabelRuns(lines []string) []string {
	out := slices.Clone(lines)
	for i := 0; i < len(out); {
		j := i
		for j < len(out) && strings.HasPrefix(out[j], "CLABEL ") {
			j++
		}
		if j > i {
			sort.Strings(out[i:j])
			i = j
		} else {
			i++
		}
	}
	return out
}

// proxyDownstream is what the proxy sent the child: its replication requests as a set (their order across charts
// varies between C runs), every other line in order (NODE_ID), then the last stream path, masked as upstream.
func proxyDownstream(down []stream.DownLine) []string {
	var requests, out []string
	last := ""
	for _, d := range down {
		switch {
		case strings.HasPrefix(d.Line, "JSON "):
			last = proxyMask(d.Line)
		case strings.HasPrefix(d.Line, "REPLAY_CHART "):
			requests = append(requests, d.Line)
		default:
			out = append(out, proxyMask(d.Line))
		}
	}
	sort.Strings(requests)
	out = append(requests, out...)
	if last != "" {
		out = append(out, "LAST PATH "+last)
	}
	return out
}

// proxyMask masks a line's proxy clocks and path times.
func proxyMask(l string) string {
	l = proxyDefEndRe.ReplaceAllString(l, "$1 T")
	l = proxyStepRe.ReplaceAllString(l, "$1 T")
	l = proxyRendRe.ReplaceAllString(l, "$1 T")
	return proxyTimesRe.ReplaceAllString(l, `"$1":T`)
}

// TestProxyTranscriptParse (daemon-free): two captures that differ only in the proxy's clocks, the path's times, the
// host labels' order and the interleaving of two charts' replication answers compare equal; a changed value, a
// missing marker or a block moved across a marker do not.
func TestProxyTranscriptParse(t *testing.T) {
	capture := func(labels, answers []string, clock, value, tail string) string {
		lines := append(slices.Clone(labels), `VARIABLE HOST proxy_marker = 0.0000000`,
			`CHART SLOT:0x1 "a" "" "a" "u" "f" "a" "line" 1000 1 "  " "p" "m"`, `CLABEL "k1" "v" 1`, `CLABEL "k2" "v" 1`,
			`CLABEL_COMMIT`, `CHART_DEFINITION_END 1 2 `+clock,
			`CHART SLOT:0x2 "b" "" "b" "u" "f" "b" "line" 1000 1 "  " "p" "m"`, `CHART_DEFINITION_END 1 2 `+clock)
		lines = append(lines, answers...)
		lines = append(lines, `JSON STREAM_PATH`, `{"since":`+clock+`,"start_time":`+clock+`}`, `JSON_PAYLOAD_END`,
			`BEGIN2 SLOT:0x1 'a' 1 3 #`, `SET2 SLOT:0x1 'x' `+value+` `+value+` A`, `END2`)
		if tail != "" {
			lines = append(lines, tail)
		}
		return strings.Join(lines, "\n") + "\n"
	}
	answer := func(chart, clock string) []string {
		return []string{`RBEGIN SLOT:1 '` + chart + `'`, `RBEGIN SLOT:1 '' 1 2 ` + clock, `RSET SLOT:1 'x' 1 A`,
			`REND 1 1 2 true  1 2 ` + clock}
	}
	labels := []string{`LABEL "k1" = 1 "v"`, `LABEL "k2" = 1 "v"`}
	marker := `VARIABLE HOST proxy_marker = 1.0000000`
	base := proxyTranscript(capture(labels, append(answer("a", "10"), answer("b", "10")...), "10", "5", marker))
	answers := append(answer("a", "10"), answer("b", "10")...)
	for name, other := range map[string]string{
		"clocks":     capture(labels, append(answer("a", "99"), answer("b", "98")...), "97", "5", marker),
		"labels":     capture([]string{labels[1], labels[0]}, answers, "10", "5", marker),
		"interleave": capture(labels, append(answer("b", "10"), answer("a", "10")...), "10", "5", marker),
		"clabels": strings.Replace(capture(labels, answers, "10", "5", marker),
			"CLABEL \"k1\" \"v\" 1\nCLABEL \"k2\" \"v\" 1\n", "CLABEL \"k2\" \"v\" 1\nCLABEL \"k1\" \"v\" 1\n", 1),
	} {
		if got := proxyTranscript(other); !slices.Equal(got, base) {
			t.Errorf("%s: differs:\n%s\nbase:\n%s", name, strings.Join(got, "\n"), strings.Join(base, "\n"))
		}
	}
	for name, other := range map[string]string{
		"value":  capture(labels, append(answer("a", "10"), answer("b", "10")...), "10", "6", marker),
		"marker": capture(labels, append(answer("a", "10"), answer("b", "10")...), "10", "5", "END2"),
	} {
		if got := proxyTranscript(other); slices.Equal(got, base) {
			t.Errorf("%s: compares equal", name)
		}
	}
	// a definition without CHART_DEFINITION_END (a parent without REPLICATION) ends at its last DIMENSION
	v1 := proxyTranscript(strings.Join([]string{`CHART "a" "" "a" "u" "f" "a" "line" 1 1 "  " "p" "m"`,
		`DIMENSION "x" "x" "absolute" 1 1 ""`, `BEGIN "a" 0`, `SET "x" = 0`, "END", marker}, "\n") + "\n")
	if want := []string{"== hooks", "== chart a", `CHART "a" "" "a" "u" "f" "a" "line" 1 1 "  " "p" "m"`,
		`DIMENSION "x" "x" "absolute" 1 1 ""`, "== stream", "DEF a", `BEGIN "a" 0`, `SET "x" = 0`, "END",
		marker, "== last path"}; !slices.Equal(v1, want) {
		t.Errorf("v1 definition:\n%s", strings.Join(v1, "\n"))
	}
	// the block after the marker instead of before it
	moved := strings.Replace(capture(labels, append(answer("a", "10"), answer("b", "10")...), "10", "5", ""),
		"JSON_PAYLOAD_END\n", "JSON_PAYLOAD_END\n"+marker+"\n", 1)
	if got := proxyTranscript(moved); slices.Equal(got, base) {
		t.Error("a block moved across a marker compares equal")
	}
}
