// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"errors"
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// The active-active identities: A and B accept the same key and stream to each other; the child streams to both.
var (
	aaA = daemon.Identity{Hostname: "parity-aa-a", StreamKey: "5a1e0000-0000-4000-8000-00000000ab02",
		MachineGUID: "5a1e0000-0000-4000-8000-00000000ab01"}
	aaB = daemon.Identity{Hostname: "parity-aa-b", StreamKey: "5a1e0000-0000-4000-8000-00000000ab02",
		MachineGUID: "5a1e0000-0000-4000-8000-00000000ab11"}
	aaChild = daemon.Identity{Hostname: "parity-aa-child", StreamKey: "5a1e0000-0000-4000-8000-00000000ab22",
		MachineGUID: "5a1e0000-0000-4000-8000-00000000ab21"}
)

// aaRows is an active-active pair with a child: A (the early parent) streams to B, B (the late peer) to A, the child
// to A and B; with `shared` every agent has the list [A B]. The form reads A-B, `r` the candidate.
func aaRows(form string, shared bool) []topoNode {
	impl := func(i int) int {
		if form[i] == 'r' {
			return 1
		}
		return 0
	}
	send := &daemon.StreamTo{Compression: true, Extra: "    reconnect delay = 5\n"}
	toA, toB := []string{"b"}, []string{"a"}
	if shared {
		toA, toB = []string{"a", "b"}, []string{"a", "b"}
	}
	return []topoNode{
		{name: "a", impl: impl(0), id: aaA, stage: 0, to: toA, send: send, opts: daemon.Options{StorageTiers: 1}},
		{name: "c", impl: 0, id: aaChild, stage: 1, to: []string{"a", "b"}, send: send,
			opts: daemon.Options{StorageTiers: 1, NoStreamKey: true}},
		{name: "b", impl: impl(2), id: aaB, stage: 2, to: toB, send: send, opts: daemon.Options{StorageTiers: 1}},
	}
}

// aaTopo is one active-active topology of the check and when it settled.
type aaTopo struct {
	form string
	*topology
	down             bool
	onlineB, settled int64
}

func (a *aaTopo) isDown() bool { return a.down }

// aaHops are the agents that hold the child, in its path's order: itself, A, then B through A.
var aaHops = []string{"c", "a", "b"}

// The outbound states each parent settles in: A sends its localhost and the child to B and has no destination for B's
// host; B sends its localhost and has none for A's host or the child (both came through A).
var (
	aaOutboundA = map[string]int{"running": 2, "no dst": 1}
	aaOutboundB = map[string]int{"running": 1, "no dst": 2}
)

// startActiveActive starts the four forms (`shared` giving every agent both parents), B once A has the child online,
// and waits until B has the child, each parent has the other, the child's path is child, A, B at every agent and the
// outbound states settle; a candidate topology that fails is marked down, the oracle's ends the test.
func startActiveActive(t *testing.T, g *stagger, shared bool) []*aaTopo {
	bins := binaries(t)
	forms := []string{"c-c", "c-r", "r-c", "r-r"}
	label := "aa"
	if shared {
		label = "aa-shared"
	}
	ts := make([]*aaTopo, len(forms))
	errs := make([]error, len(forms))
	var wg sync.WaitGroup
	for i, form := range forms {
		// what a goroutine that ends early (a helper's Fatal) leaves: a topology that is down
		ts[i], errs[i] = &aaTopo{form: form, down: true}, fmt.Errorf("%s did not start", form)
		wg.Add(1)
		go func() {
			defer wg.Done()
			tp, err := startTopology(t, g, label+"-"+form, bins, aaRows(form, shared), func(tp *topology, stage int) error {
				if stage == 2 {
					if _, b, ok := waitHop(tp.addr("a"), aaChild.MachineGUID, true, 120*time.Second); !ok {
						return fmt.Errorf("%s: A never had the child online: %.300s", form, b)
					}
				}
				return nil
			})
			ts[i], errs[i] = &aaTopo{form: form, topology: tp}, err
		}()
	}
	wg.Wait()
	for i, err := range errs {
		if err == nil {
			continue
		}
		if i == 0 {
			t.Fatalf("the oracle topology: %v", err)
		}
		t.Errorf("topology %s: %v", forms[i], err)
		ts[i].down = true
	}
	want := []string{aaChild.Hostname, aaA.Hostname, aaB.Hostname}
	forEach(ts, func(a *aaTopo) {
		for _, hop := range []struct{ at, guid string }{{"b", aaChild.MachineGUID}, {"a", aaB.MachineGUID},
			{"b", aaA.MachineGUID}} {
			if _, b, ok := waitHop(a.addr(hop.at), hop.guid, true, 150*time.Second); !ok {
				t.Errorf("%s: %s never had %s online: %.300s", a.form, hop.at, hop.guid, b)
				a.down = true
				return
			}
			if hop.guid == aaChild.MachineGUID {
				a.onlineB = time.Now().Unix()
			}
		}
		for _, at := range []struct{ node, query string }{{"a", "&nodes=" + aaChild.Hostname},
			{"b", "&nodes=" + aaChild.Hostname}, {"c", ""}} {
			if got, ok := waitPath(a.addr(at.node), at.query, want, 30*time.Second); !ok {
				t.Errorf("%s: %s's path of the child is %v, not %v", a.form, at.node, got, want)
			}
		}
		for _, w := range []struct {
			node string
			want map[string]int
		}{{"a", aaOutboundA}, {"b", aaOutboundB}} {
			if err := waitCounts(a.d(w.node), "netdata.streaming_outbound", w.want, 60*time.Second); err != nil {
				t.Errorf("%s: %s's outbound states: %v", a.form, w.node, err)
			}
		}
		a.settled = time.Now().Unix()
	})
	if ts[0].down {
		t.FailNow()
	}
	return ts
}

// waitPath polls once a second until the first node of an agent's /api/v3/stream_path (with `query`) has entries
// with these hostnames in this order, and returns the last hostnames seen.
func waitPath(addr, query string, want []string, limit time.Duration) ([]string, bool) {
	var got []string
	for end := time.Now().Add(limit); time.Now().Before(end); time.Sleep(time.Second) {
		b, err := rawExchange(addr, []byte("GET /api/v3/stream_path?options=minify"+query+" HTTP/1.1\r\n\r\n"),
			5*time.Second)
		if err != nil {
			continue
		}
		nodes, err := pathEntries(httpBody(b))
		if err != nil || len(nodes) == 0 {
			continue
		}
		got = got[:0]
		for _, e := range nodes[0] {
			got = append(got, e.Hostname)
		}
		if slices.Equal(got, want) {
			return got, true
		}
	}
	return got, false
}

// maxPathHops is the most hops any entry of any node's path has in an agent's /api/v3/stream_path.
func maxPathHops(addr string) (int, error) {
	b, err := rawExchange(addr, []byte("GET /api/v3/stream_path?options=minify HTTP/1.1\r\n\r\n"), 5*time.Second)
	if err != nil {
		return 0, err
	}
	nodes, err := pathHops(httpBody(b))
	if err != nil {
		return 0, err
	}
	most := 0
	for _, n := range nodes {
		for _, h := range n {
			most = max(most, h)
		}
	}
	return most, nil
}

// wantCounts checks every row of a CSV answer has each dimension of `want` at its value and every other at 0.
func wantCounts(csv string, want map[string]int) error {
	lines := strings.Split(strings.TrimSpace(csv), "\n")
	header, rows := lines[0], lines[1:]
	names := strings.Split(header, ",")
	for _, name := range slices.Sorted(maps.Keys(want)) {
		if !slices.Contains(names, name) {
			return fmt.Errorf("no dimension %q in %q", name, header)
		}
	}
	if len(rows) == 0 {
		return errors.New("no rows")
	}
	for _, r := range rows {
		cells := strings.Split(r, ",")
		for i := 1; i < len(names) && i < len(cells); i++ {
			if cells[i] != strconv.Itoa(want[names[i]]) {
				return fmt.Errorf("row %q, header %q", r, header)
			}
		}
	}
	return nil
}

// waitCounts waits, up to `limit`, until three answers in a row of an agent's chart over its last three seconds
// have the counts of `want` (wantCounts).
func waitCounts(d *daemon.Daemon, chart string, want map[string]int, limit time.Duration) error {
	var last error
	good := 0
	for end := time.Now().Add(limit); time.Now().Before(end); time.Sleep(time.Second) {
		now := time.Now().Unix()
		csv, err := localDataErr(d, chart, now-3, now-1, "average")
		if err == nil {
			err = wantCounts(csv, want)
		}
		if last = err; err != nil {
			good = 0
			continue
		}
		if good++; good == 3 {
			return nil
		}
	}
	return last
}

// logCount is how many records of an agent's daemon log contain `substr`.
func logCount(d *daemon.Daemon, substr string) (int, error) {
	b, err := os.ReadFile(filepath.Join(d.Opts.RunDir, "log", "daemon.log"))
	if err != nil {
		return 0, err
	}
	return bytes.Count(b, []byte(substr)), nil
}

// portNames rewrites the topology's local addresses to its nodes' names, so records that name a destination compare
// across topologies.
func (tp *topology) portNames() *strings.Replacer {
	var pairs []string
	for name, port := range tp.port {
		pairs = append(pairs, "127.0.0.1:"+strconv.Itoa(port), name)
	}
	return strings.NewReplacer(pairs...)
}

// topoRecords are an agent's records that contain `marker`, with the topology's addresses as node names, masked
// (maskRecordSet).
func topoRecords(t *testing.T, tp *topology, node, marker string) []string {
	t.Helper()
	return maskRecordSet(parentRecords(t, tp.d(node), marker, tp.portNames()))
}

// The views of the active-active paths: the child at each parent and its own, each parent's own and its peer's.
var (
	aaChildViews = [][2]string{
		{"a", "/api/v3/stream_path?nodes=parity-aa-child&options=minify"},
		{"b", "/api/v3/stream_path?nodes=parity-aa-child&options=minify"},
		{"c", "/api/v3/stream_path?options=minify"},
	}
	aaPeerViews = [][2]string{
		{"a", "/api/v3/stream_path?nodes=parity-aa-a&options=minify"},
		{"b", "/api/v3/stream_path?nodes=parity-aa-b&options=minify"},
		{"b", "/api/v3/stream_path?nodes=parity-aa-a&options=minify"},
	}
	// B's labels at A: B's `_is_parent` goes up at its localhost's READY, racing the child's arrival through A
	aaBAtAView = [2]string{"a", "/api/v3/stream_path?nodes=parity-aa-b&options=minify"}
)

// TestStreamActiveActive (check `stream.active-active`, milestone 7 commit 9, D109.5, D122.1-3; plan
// `evidence/2026-09-30-plan-m7-commit9.md`, re-grounded `evidence/2026-10-01-regrounding-m7-commit9.md`): two parents
// A and B that stream to each other with a child streaming to both, in four forms run side by side (c-c the oracle,
// c-r, r-c, r-r). The child connects to A (B is not started yet, so its probe of B is refused and B is blocked); A
// proxies it to B; B's proxy for the child and A's for B's host find their destination in the path before them and
// ban it. `default`: the paths, stream_info, the outbound and inbound states, the child's data and charts at both
// parents, each parent's view of the other, the records.
func TestStreamActiveActive(t *testing.T) {
	t.Run("default", testActiveActiveDefault)
}

func testActiveActiveDefault(t *testing.T) {
	g := &stagger{gap: 2 * time.Second}
	ts := startActiveActive(t, g, false)
	oracle := ts[0]

	// the paths: no entry farther than two hops anywhere
	forEach(ts, func(a *aaTopo) {
		for _, node := range []string{"a", "b", "c"} {
			if most, err := maxPathHops(a.addr(node)); err != nil || most > 2 {
				t.Errorf("%s: %s's paths reach %d hops (%v)", a.form, node, most, err)
			}
		}
	})
	for _, a := range ts[1:] {
		if a.down {
			continue
		}
		t.Run(a.form+"/views", func(t *testing.T) {
			topoViews(t, "views", oracle.topology, a.topology, append(slices.Clone(aaChildViews), aaPeerViews...),
				entryTimes)
			topoViews(t, "views", oracle.topology, a.topology, [][2]string{aaBAtAView}, entryTimes, "_is_parent")
			for _, at := range []string{"a", "b"} {
				o, oerr := streamInfoMasked(oracle.addr(at), aaChild.MachineGUID)
				got, gerr := streamInfoMasked(a.addr(at), aaChild.MachineGUID)
				if oerr != nil || gerr != nil || !bytes.Equal(o, got) {
					t.Errorf("%s stream_info of the child: %v %v %s", at, oerr, gerr, firstDifference(o, got))
				}
				peer := map[string]string{"a": aaB.MachineGUID, "b": aaA.MachineGUID}[at]
				o, oerr = streamInfoMasked(oracle.addr(at), peer)
				got, gerr = streamInfoMasked(a.addr(at), peer)
				if oerr != nil || gerr != nil || !bytes.Equal(o, got) {
					t.Errorf("%s stream_info of its peer: %v %v %s", at, oerr, gerr, firstDifference(o, got))
				}
			}
		})
	}

	// the outbound and inbound states, over the same three seconds
	now := time.Now().Unix()
	for _, a := range ts[1:] {
		if a.down {
			continue
		}
		t.Run(a.form+"/states", func(t *testing.T) {
			for _, node := range []string{"a", "b"} {
				for _, chart := range []string{"netdata.streaming_outbound", "netdata.netdata.streaming_inbound_permanent"} {
					o := localData(t, oracle.d(node), chart, now-3, now-1, "average")
					got := localData(t, a.d(node), chart, now-3, now-1, "average")
					if o != got {
						t.Errorf("%s %s: %s", node, chart, firstDifference([]byte(o), []byte(got)))
					}
				}
			}
		})
	}

	// the child's data within each topology: exact over the live window at B, tolerant from its first point
	settled := oracle.settled
	for _, a := range ts {
		if !a.down {
			settled = max(settled, a.settled)
		}
	}
	time.Sleep(time.Until(time.Unix(settled+30, 0)))
	for _, a := range ts {
		if a.down {
			continue
		}
		t.Run(a.form+"/data", func(t *testing.T) {
			for _, chart := range chainDataCharts {
				last := hopLast(t, a.topology, aaChild.Hostname, aaHops, chart) - 1
				live := hopSeries(t, a.topology, aaChild.Hostname, aaHops, chart, a.onlineB+5, last)
				for j := 1; j < len(live); j++ {
					if live[j-1] != live[j] {
						t.Errorf("%s live, hop %d: %s", chart, j, firstDifference([]byte(live[j-1]), []byte(live[j])))
					}
				}
				if _, rows := csvRows(live[0]); len(rows) < 20 {
					t.Errorf("%s live: %d rows", chart, len(rows))
				}
				first := jsonNumber(t, a.d("c"), "/api/v1/chart?chart="+chart, "first_entry")
				compareHops(t, chart+" full", hopSeries(t, a.topology, aaChild.Hostname, aaHops, chart, first, last))
			}
		})
	}

	// the charts: the child's at both parents (equal there first), each parent's of its peer; the topology's addresses
	// (a parent's `_streams_to` label) by node name
	charts := func(t *testing.T, a *aaTopo, node, host string) []byte {
		t.Helper()
		return []byte(a.portNames().Replace(string(hopCharts(t, a.d(node), host, false))))
	}
	settle := func(t *testing.T, a *aaTopo, at [2]string, host string) []byte {
		t.Helper()
		for end := time.Now().Add(15 * time.Second); ; time.Sleep(time.Second) {
			x, y := charts(t, a, at[0], host), charts(t, a, at[1], host)
			if bytes.Equal(x, y) || time.Now().After(end) {
				return y
			}
		}
	}
	for _, a := range ts[1:] {
		if a.down {
			continue
		}
		t.Run(a.form+"/charts", func(t *testing.T) {
			o := settle(t, oracle, [2]string{"a", "b"}, aaChild.Hostname)
			if got := settle(t, a, [2]string{"a", "b"}, aaChild.Hostname); !bytes.Equal(o, got) {
				t.Errorf("B's charts of the child: %s", firstDifference(o, got))
			}
			for _, peer := range []struct{ at, host string }{{"a", aaB.Hostname}, {"b", aaA.Hostname}} {
				o := charts(t, oracle, peer.at, peer.host)
				if got := charts(t, a, peer.at, peer.host); !bytes.Equal(o, got) {
					t.Errorf("%s's charts of %s: %s", peer.at, peer.host, firstDifference(o, got))
				}
			}
		})
	}

	// the records, as sets: the bans, both parents' receivers, the child's; no second connection of a host anywhere
	records := func(a *aaTopo) map[string][]string {
		return map[string][]string{
			"a bans":     topoRecords(t, a.topology, "a", "is banned"),
			"b bans":     topoRecords(t, a.topology, "b", "is banned"),
			"a receiver": topoRecords(t, a.topology, "a", "STREAM RCV"),
			"b receiver": topoRecords(t, a.topology, "b", "STREAM RCV"),
			"child":      rchildRecords(t, a.d("c")),
		}
	}
	for _, a := range ts {
		if a.down {
			continue
		}
		for _, node := range []string{"a", "b", "c"} {
			if n, err := logCount(a.d(node), "multiple connections for the same host"); err != nil || n != 0 {
				t.Errorf("%s: %s has %d records of a second connection (%v)", a.form, node, n, err)
			}
		}
	}
	want := records(oracle)
	for _, name := range slices.Sorted(maps.Keys(want)) {
		t.Logf("oracle %s records:\n%s", name, strings.Join(want[name], "\n"))
	}
	for _, name := range []string{"a bans", "b bans"} {
		if len(want[name]) == 0 {
			t.Errorf("the oracle's %s: none", name)
		}
	}
	for _, a := range ts[1:] {
		if !a.down {
			t.Run(a.form+"/records", func(t *testing.T) { compareRecordSets(t, want, records(a)) })
		}
	}
}
