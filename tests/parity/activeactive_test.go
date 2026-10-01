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
	down                           bool
	onlineB, settled, onPeer, back int64
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

// aaForms are the four forms of A and B, the oracle first.
var aaForms = []string{"c-c", "c-r", "r-c", "r-r"}

// startActiveActive starts the forms (`shared` giving every agent both parents), B once A has the child online,
// and waits until B has the child, each parent has the other, the child's path is child, A, B at every agent and the
// outbound states settle; a candidate topology that fails is marked down, the oracle's ends the test.
func startActiveActive(t *testing.T, g *stagger, shared bool, forms []string) []*aaTopo {
	bins := binaries(t)
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
	t.Run("shared-list", testActiveActiveSharedList)
	t.Run("kill-direct", testActiveActiveKillDirect)
}

// testActiveActiveSharedList (case `shared-list`, 9c): every agent has the list [A B], c-c and r-r. Each parent's
// localhost bans itself as the origin; its proxies ban whichever destination the path has before it, the origin of
// the host they relay, and themselves: A four bans, B five. Compared as `default`'s paths, states and records.
func testActiveActiveSharedList(t *testing.T) {
	g := &stagger{gap: 2 * time.Second}
	ts := startActiveActive(t, g, true, []string{"c-c", "r-r"})
	aaCompareViews(t, ts)
	aaCompareStates(t, ts)
	aaCompareRecords(t, ts, map[string]int{"a bans": 4, "b bans": 5})
}

func testActiveActiveDefault(t *testing.T) {
	g := &stagger{gap: 2 * time.Second}
	ts := startActiveActive(t, g, false, aaForms)
	aaCompareViews(t, ts)
	aaCompareStates(t, ts)
	aaCompareData(t, ts)
	aaCompareRecords(t, ts, map[string]int{"a bans": 1, "b bans": 2})
}

// aaCompareViews compares the topologies' paths and stream_info with the oracle's (the first); no path anywhere
// reaches three hops.
func aaCompareViews(t *testing.T, ts []*aaTopo) {
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

}

// aaCompareStates compares the parents' outbound and inbound states with the oracle's over the same three seconds.
func aaCompareStates(t *testing.T, ts []*aaTopo) {
	oracle := ts[0]
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

}

// aaCompareData compares, within each topology, the child's series at the child, A and B (exact over the live window
// from B's online, tolerant from the child's first point), then the child's charts at B and each parent's of its peer
// with the oracle's.
func aaCompareData(t *testing.T, ts []*aaTopo) {
	oracle := ts[0]
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

}

// aaCompareRecords compares the records with the oracle's as sets: the bans (the oracle's counted as `bans`), both
// parents' receivers, the child's; no agent logs a second connection of a host.
func aaCompareRecords(t *testing.T, ts []*aaTopo, bans map[string]int) {
	oracle := ts[0]
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
	for name, n := range bans {
		if len(want[name]) != n {
			t.Errorf("the oracle's %s: %d, want %d", name, len(want[name]), n)
		}
	}
	for _, a := range ts[1:] {
		if !a.down {
			t.Run(a.form+"/records", func(t *testing.T) { compareRecordSets(t, want, records(a)) })
		}
	}
}

// waitEntryHops polls once a second until the first node of an agent's /api/v3/stream_path (with `query`) has the
// host at these hops.
func waitEntryHops(addr, query, hostname string, hops int, limit time.Duration) bool {
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
		if slices.ContainsFunc(nodes[0], func(e pathEntry) bool { return e.Hostname == hostname && e.Hops == hops }) {
			return true
		}
	}
	return false
}

// testActiveActiveKillDirect (case `kill-direct`, 9d, PARITY_LONG, D109.6, D109.7): A, the child's parent, is
// SIGKILLed in every topology once steady; the child moves to B; A, relaunched once the child is on B in every
// topology, gets the child back through B. The roles swap: B sends its localhost and the child to A, and A has no
// destination for either. Compared with c-c: B's views while A is down, then the paths, stream_info and states, the
// child's series at B over the move and at A after its return, and the records with a kill's forms masked.
func testActiveActiveKillDirect(t *testing.T) {
	if os.Getenv("PARITY_LONG") == "" {
		t.Skip("PARITY_LONG unset (a kill and a relaunch run for minutes)")
	}
	g := &stagger{gap: 2 * time.Second}
	ts := startActiveActive(t, g, false, aaForms)
	oracle := ts[0]
	k := oracle.settled
	for _, a := range ts {
		if !a.down {
			k = max(k, a.settled)
		}
	}
	k += 20
	time.Sleep(time.Until(time.Unix(k, 0)))
	forEach(ts, func(a *aaTopo) {
		if err := a.d("a").Kill(); err != nil {
			t.Errorf("%s: kill A: %v", a.form, err)
			a.down = true
		}
	})
	if oracle.down {
		t.FailNow()
	}

	// the child moves to B: B at one hop in its own path
	forEach(ts, func(a *aaTopo) {
		if !waitEntryHops(a.addr("c"), "", aaB.Hostname, 1, 125*time.Second) {
			t.Errorf("%s: the child never moved to B", a.form)
			a.down = true
			return
		}
		a.onPeer = time.Now().Unix()
		n, _ := logCount(a.d("b"), "multiple connections for the same host")
		t.Logf("%s: the child on B %d s after the kill; B refused it as a second connection %d times", a.form,
			a.onPeer-k, n)
	})
	if oracle.down {
		t.FailNow()
	}
	r := oracle.onPeer
	for _, a := range ts {
		if !a.down {
			r = max(r, a.onPeer)
		}
	}
	r += 5

	// while A is down: B's views of the child and of A's host, and the child's own
	time.Sleep(time.Until(time.Unix(r-1, 0)))
	down := [][2]string{aaChildViews[1], aaChildViews[2], aaPeerViews[2]}
	for _, a := range ts[1:] {
		if !a.down {
			t.Run(a.form+"/down", func(t *testing.T) {
				topoViews(t, "down", oracle.topology, a.topology, down, entryKillTimes)
			})
		}
	}

	// A back, after the child is on B everywhere: it gets the child through B, and the roles swap
	time.Sleep(time.Until(time.Unix(r, 0)))
	want := []string{aaChild.Hostname, aaB.Hostname, aaA.Hostname}
	forEach(ts, func(a *aaTopo) {
		g.wait()
		if err := relaunch(a.d("a")); err != nil {
			t.Errorf("%s: relaunch A: %v", a.form, err)
			a.down = true
			return
		}
		if _, b, ok := waitHop(a.addr("a"), aaChild.MachineGUID, true, 200*time.Second); !ok {
			t.Errorf("%s: A never had the child back: %.300s", a.form, b)
			a.down = true
			return
		}
		a.back = time.Now().Unix()
		t.Logf("%s: A has the child back %d s after the kill", a.form, a.back-k)
		// B's and the child's own; the restarted A keeps the child's entries it loaded (C: child, A, B), which the
		// views compare (D109.6)
		for _, at := range []struct{ node, query string }{{"b", "&nodes=" + aaChild.Hostname}, {"c", ""}} {
			if got, ok := waitPath(a.addr(at.node), at.query, want, 60*time.Second); !ok {
				t.Errorf("%s: %s's path of the child is %v, not %v", a.form, at.node, got, want)
			}
		}
		for _, w := range []struct {
			node string
			want map[string]int
		}{{"b", aaOutboundA}, {"a", aaOutboundB}} {
			if err := waitCounts(a.d(w.node), "netdata.streaming_outbound", w.want, 90*time.Second); err != nil {
				t.Errorf("%s: %s's outbound states: %v", a.form, w.node, err)
			}
		}
	})
	if oracle.down {
		t.FailNow()
	}

	// after: the paths (A's start median masked: it restarted), stream_info and states as `default`'s
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
		t.Run(a.form+"/final", func(t *testing.T) {
			topoViews(t, "final", oracle.topology, a.topology, append(slices.Clone(aaChildViews), aaPeerViews[:2]...),
				entryKillTimes)
			// each parent's labels at the other go up at its READY, racing its children's arrival
			topoViews(t, "final", oracle.topology, a.topology, [][2]string{aaBAtAView, aaPeerViews[2]}, entryKillTimes,
				"_is_parent")
			for _, at := range []string{"a", "b"} {
				o, oerr := streamInfoMasked(oracle.addr(at), aaChild.MachineGUID)
				got, gerr := streamInfoMasked(a.addr(at), aaChild.MachineGUID)
				if oerr != nil || gerr != nil || !bytes.Equal(o, got) {
					t.Errorf("%s stream_info of the child: %v %v %s", at, oerr, gerr, firstDifference(o, got))
				}
			}
		})
	}
	aaCompareStates(t, ts)

	// the child's data within each topology: at B over the move (the child never stopped), at A after its return
	for _, a := range ts {
		if a.down {
			continue
		}
		t.Run(a.form+"/data", func(t *testing.T) {
			waitHopsReach(t, a.topology, aaChild.Hostname, aaHopsBack, chainDataCharts, a.back+10, 30*time.Second)
			for _, chart := range chainDataCharts {
				move := hopSeries(t, a.topology, aaChild.Hostname, []string{"c", "b"}, chart, k-20, a.onPeer+10)
				if _, rows := csvRows(move[0]); len(rows) < 25 || slices.ContainsFunc(rows, nullRow) {
					t.Errorf("%s: the child's own series over [%d, %d] has %d rows, nulls %q", chart, k-20,
						a.onPeer+10, len(rows), slices.DeleteFunc(slices.Clone(rows), func(r string) bool { return !nullRow(r) }))
				}
				compareHops(t, chart+" over the move", move)
				last := hopLast(t, a.topology, aaChild.Hostname, aaHopsBack, chart) - 1
				compareHops(t, chart+" back", hopSeries(t, a.topology, aaChild.Hostname, aaHopsBack, chart,
					a.back+5, last))
			}
		})
	}

	// the records of the kill, as sets: the bans, B's removal of the hosts A had sent, the receivers, the child's;
	// a second connection the child meets at B while B still has it through A is timing (D109.7): dropped
	records := func(a *aaTopo) map[string][]string {
		recs := map[string][]string{
			"a bans":     topoRecords(t, a.topology, "a", "is banned"),
			"b bans":     topoRecords(t, a.topology, "b", "is banned"),
			"b removed":  topoRecords(t, a.topology, "b", "streaming connector removed host"),
			"a receiver": topoRecords(t, a.topology, "a", "STREAM RCV"),
			"b receiver": topoRecords(t, a.topology, "b", "STREAM RCV"),
			"child":      rchildRecords(t, a.d("c")),
		}
		for name, lines := range recs {
			lines = slices.DeleteFunc(lines, func(l string) bool {
				return strings.Contains(l, "multiple connections for the same host")
			})
			recs[name] = killRecords(lines)
		}
		return recs
	}
	wantRecs := records(oracle)
	for _, name := range slices.Sorted(maps.Keys(wantRecs)) {
		t.Logf("oracle %s records:\n%s", name, strings.Join(wantRecs[name], "\n"))
	}
	for _, a := range ts[1:] {
		if !a.down {
			t.Run(a.form+"/records", func(t *testing.T) { compareRecordSets(t, wantRecs, records(a)) })
		}
	}
}

// aaHopsBack are the agents that hold the child once A is back: itself, B, then A through B.
var aaHopsBack = []string{"c", "b", "a"}
