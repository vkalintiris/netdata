// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"maps"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// topoNode is one agent of a topology: its name, which binary it runs (0 the oracle's, 1 the candidate's), its
// identity, its start stage, the nodes it streams to (in order) with its [stream] template, and its other options.
type topoNode struct {
	name  string
	impl  int
	id    daemon.Identity
	stage int
	to    []string
	send  *daemon.StreamTo
	opts  daemon.Options
}

// topology is a set of agents started from a node table, each on a port picked before any starts, so destinations
// are known by name.
type topology struct {
	label string
	rows  []topoNode
	nodes map[string]*daemon.Daemon
	port  map[string]int
}

func (tp *topology) d(name string) *daemon.Daemon { return tp.nodes[name] }

func (tp *topology) addr(name string) string { return "127.0.0.1:" + strconv.Itoa(tp.port[name]) }

// stop stops every started agent, the last stage first.
func (tp *topology) stop() {
	for i := len(tp.rows) - 1; i >= 0; i-- {
		if d := tp.nodes[tp.rows[i].name]; d != nil {
			_ = d.Stop()
		}
	}
}

// stagger spaces the launches of every topology in a test: a C child that connects before its system-info script
// ends on a busy host sends the fields empty.
type stagger struct {
	mu   sync.Mutex
	gap  time.Duration
	last time.Time
}

func (s *stagger) wait() {
	s.mu.Lock()
	defer s.mu.Unlock()
	if d := time.Until(s.last.Add(s.gap)); d > 0 {
		time.Sleep(d)
	}
	s.last = time.Now()
}

// startTopology starts the rows by stage (in table order within one), `before` running ahead of each stage after
// the first; a port another process took meanwhile restarts the whole set on new ports (three attempts). It
// returns errors rather than failing the test, as topologies start in goroutines.
func startTopology(t *testing.T, g *stagger, label string, bins [2]string, rows []topoNode,
	before func(tp *topology, stage int) error) (*topology, error) {
	var lastErr error
	for attempt := 1; attempt <= 3; attempt++ {
		ports := reservePorts(t, len(rows))
		tp := &topology{label: label, rows: rows, nodes: map[string]*daemon.Daemon{}, port: map[string]int{}}
		for i, r := range rows {
			tp.port[r.name] = ports[i]
		}
		lastErr = tp.start(t, g, bins, attempt, before)
		if lastErr == nil {
			return tp, nil
		}
		tp.stop()
		if !errors.Is(lastErr, daemon.ErrPortTaken) {
			return nil, lastErr
		}
		t.Logf("%s: %v; new ports", label, lastErr)
	}
	return nil, lastErr
}

// reserved are the ports handed to topologies in this test process: freePorts releases what it picked before a
// daemon binds it, so topologies starting together could otherwise get the same port.
var reserved = struct {
	sync.Mutex
	ports map[int]bool
}{ports: map[int]bool{}}

// reservePorts is freePorts without a port another topology of the process already holds.
func reservePorts(t *testing.T, n int) []int {
	reserved.Lock()
	defer reserved.Unlock()
	for {
		ports := freePorts(t, n)
		if !slices.ContainsFunc(ports, func(p int) bool { return reserved.ports[p] }) {
			for _, p := range ports {
				reserved.ports[p] = true
			}
			return ports
		}
	}
}

func (tp *topology) start(t *testing.T, g *stagger, bins [2]string, attempt int,
	before func(tp *topology, stage int) error) error {
	suffix := ""
	if attempt > 1 {
		suffix = "-" + strconv.Itoa(attempt)
	}
	stages := 0
	for _, r := range tp.rows {
		stages = max(stages, r.stage+1)
	}
	for stage := range stages {
		if stage > 0 && before != nil {
			if err := before(tp, stage); err != nil {
				return err
			}
		}
		for _, r := range tp.rows {
			if r.stage != stage {
				continue
			}
			o := r.opts
			id := r.id
			o.Binary = bins[r.impl]
			o.Port = tp.port[r.name]
			o.Identity = &id
			o.RunDir = runDir(t, Role(tp.label+"-"+r.name+suffix))
			if len(r.to) > 0 {
				send := *r.send
				var dests []string
				for _, to := range r.to {
					dests = append(dests, tp.addr(to))
				}
				send.Destination = strings.Join(dests, " ")
				send.APIKey = tp.row(r.to[0]).id.StreamKey
				o.StreamTo = &send
			}
			g.wait()
			d, err := daemon.Start(o)
			if err != nil {
				return fmt.Errorf("%s %s: %w", tp.label, r.name, err)
			}
			tp.nodes[r.name] = d
			t.Cleanup(func() {
				if err := d.Stop(); err != nil {
					t.Errorf("%s %s: stop: %v", tp.label, r.name, err)
				}
			})
		}
	}
	return nil
}

func (tp *topology) row(name string) topoNode {
	for _, r := range tp.rows {
		if r.name == name {
			return r
		}
	}
	panic("no row " + name)
}

// The chain's identities, the same in every chain (separate processes and ports).
var (
	chainGP = daemon.Identity{Hostname: "parity-chain-gp", StreamKey: "5a1e0000-0000-4000-8000-00000000ca02",
		MachineGUID: "5a1e0000-0000-4000-8000-00000000ca01"}
	chainProxy = daemon.Identity{Hostname: "parity-chain-proxy", StreamKey: "5a1e0000-0000-4000-8000-00000000ca12",
		MachineGUID: "5a1e0000-0000-4000-8000-00000000ca11"}
	chainChild = daemon.Identity{Hostname: "parity-chain-child", StreamKey: "5a1e0000-0000-4000-8000-00000000ca22",
		MachineGUID: "5a1e0000-0000-4000-8000-00000000ca21"}
)

// chainRows is a child → proxy → grandparent chain; the form reads child-proxy-grandparent, `r` the candidate.
func chainRows(form string) []topoNode {
	impl := func(i int) int {
		if form[i] == 'r' {
			return 1
		}
		return 0
	}
	send := &daemon.StreamTo{Compression: true, Extra: "    reconnect delay = 5\n"}
	return []topoNode{
		{name: "gp", impl: impl(4), id: chainGP, stage: 0,
			opts: daemon.Options{StorageTiers: 1, StreamMemoryMode: "dbengine"}},
		{name: "p", impl: impl(2), id: chainProxy, stage: 1, to: []string{"gp"}, send: send,
			opts: daemon.Options{StorageTiers: 1}},
		{name: "c", impl: impl(0), id: chainChild, stage: 2, to: []string{"p"}, send: send,
			opts: daemon.Options{StorageTiers: 1, NoStreamKey: true}},
	}
}

// receiverOnline tells whether an agent's stream_info says the host (by machine GUID) streams to it and is online.
func receiverOnline(addr, guid string) (bool, []byte) {
	b, err := rawExchange(addr, []byte("GET /api/v3/stream_info?machine_guid="+guid+" HTTP/1.1\r\n\r\n"), 5*time.Second)
	return err == nil && bytes.Contains(b, []byte(`"ingest_type":"child"`)) &&
		bytes.Contains(b, []byte(`"ingest_status":"online"`)), b
}

// waitHop polls every 500 ms until an agent has the host online (`want`) or no longer as a child (`!want`: a
// failed request or a transient state such as replicating does not count as gone).
func waitHop(addr, guid string, want bool, limit time.Duration) (time.Duration, []byte, bool) {
	start := time.Now()
	var last []byte
	for time.Since(start) < limit {
		var ok bool
		ok, last = receiverOnline(addr, guid)
		gone := bytes.Contains(last, []byte(`"ingest_type":`)) && !bytes.Contains(last, []byte(`"ingest_type":"child"`))
		if (want && ok) || (!want && gone) {
			return time.Since(start), last, true
		}
		time.Sleep(500 * time.Millisecond)
	}
	return time.Since(start), last, false
}

// streamInfoMasked is an agent's /api/v3/stream_info of a host with its retention and nonce masked.
func streamInfoMasked(addr, guid string) ([]byte, error) {
	b, err := rawExchange(addr, []byte("GET /api/v3/stream_info?machine_guid="+guid+" HTTP/1.1\r\n\r\n"), 5*time.Second)
	if err != nil {
		return nil, err
	}
	out := streamInfoRetentionRe.ReplaceAll(httpBody(b), []byte(`"${1}":T`))
	return streamInfoNonceRe.ReplaceAll(out, []byte(`"nonce":N`)), nil
}

// pathHops are the hops of each node's stream path in a /api/v3/stream_path answer.
func pathHops(body []byte) ([][]int, error) {
	var v struct {
		Nodes []struct {
			Path []struct {
				Hops int `json:"hops"`
			} `json:"streaming_path"`
		} `json:"nodes"`
	}
	if err := json.Unmarshal(body, &v); err != nil {
		return nil, err
	}
	var out [][]int
	for _, n := range v.Nodes {
		var hops []int
		for _, e := range n.Path {
			hops = append(hops, e.Hops)
		}
		out = append(out, hops)
	}
	return out, nil
}

// waitPathHops waits until the first node of an agent's /api/v3/stream_path (with `query`) has entries with these
// hops, and returns the last hops seen.
func waitPathHops(addr, query string, want []int, limit time.Duration) ([]int, bool) {
	var hops []int
	for end := time.Now().Add(limit); time.Now().Before(end); time.Sleep(time.Second) {
		b, err := rawExchange(addr, []byte("GET /api/v3/stream_path?options=minify"+query+" HTTP/1.1\r\n\r\n"),
			5*time.Second)
		if err != nil {
			continue
		}
		all, err := pathHops(httpBody(b))
		if err != nil || len(all) == 0 {
			continue
		}
		hops = all[0]
		if slices.Equal(hops, want) {
			return hops, true
		}
	}
	return hops, false
}

// chain is one topology of the check and when its child came online, stopped and returned.
type chain struct {
	form string
	*topology
	down                      bool
	online, stopped, returned int64
}

// forChains runs f on every chain that is up, in parallel; f may only report with t.Errorf.
func forChains(cs []*chain, f func(c *chain)) {
	var wg sync.WaitGroup
	for _, c := range cs {
		if c.down {
			continue
		}
		wg.Add(1)
		go func() {
			defer wg.Done()
			f(c)
		}()
	}
	wg.Wait()
}

// chainViews compares the stream path views of two chains' agents: each view is a node name and a request;
// `_streams_to` and `maskLabels` are compared by presence only.
func chainViews(t *testing.T, stage string, oracle, cand *chain, views [][2]string, times *regexp.Regexp,
	maskLabels ...string) {
	t.Helper()
	for _, v := range views {
		compareStreamPathWith(t, stage+" "+v[0], [2]string{oracle.addr(v[0]), cand.addr(v[0])},
			[2]*strings.Replacer{}, v[1], times, append([]string{"_streams_to"}, maskLabels...)...)
	}
}

// The views of the chain's paths: the child and the proxy at the grandparent and at the proxy, and the child's own.
var (
	gpChildView    = [2]string{"gp", "/api/v3/stream_path?nodes=parity-chain-child&options=minify"}
	gpProxyView    = [2]string{"gp", "/api/v3/stream_path?nodes=parity-chain-proxy&options=minify"}
	proxyChildView = [2]string{"p", "/api/v3/stream_path?nodes=parity-chain-child&options=minify"}
	proxyOwnView   = [2]string{"p", "/api/v3/stream_path?nodes=parity-chain-proxy&options=minify"}
	childOwnView   = [2]string{"c", "/api/v3/stream_path?options=minify"}
	chainPathViews = [][2]string{gpChildView, gpProxyView, proxyChildView, proxyOwnView, childOwnView}
)

// entryRestartTimes also masks the timing medians a path entry carries, which a restart changes.
var entryRestartTimes = regexp.MustCompile(`("since":)\d+(,\s*"first_time_t":)\d+,\s*"start_time":\d+,\s*"shutdown_time":\d+`)

// hopCharts is an agent's /api/v1/charts of the chain's child, as parentCharts normalizes it, with `hosts` (every
// host on the agent) only when `withHosts`.
func hopCharts(t *testing.T, d *daemon.Daemon, withHosts bool) []byte {
	t.Helper()
	b, err := rawExchange(d.Addr, []byte("GET /host/parity-chain-child/api/v1/charts HTTP/1.1\r\n\r\n"), 10*time.Second)
	if err != nil {
		t.Fatalf("%s: charts: %v", d.Opts.RunDir, err)
	}
	out := parentCharts(t, httpBody(b))
	if withHosts {
		return out
	}
	var v map[string]any
	if err := json.Unmarshal(out, &v); err != nil {
		t.Fatal(err)
	}
	delete(v, "hosts")
	delete(v, "hosts_count")
	out, _ = json.Marshal(v)
	return out
}

// chainDataCharts are the charts whose series the check compares across the hops.
var chainDataCharts = append(append([]string{}, gapCharts...), "netdata.memory")

// chainLast is the newest point every hop of a chain has of a chart.
func chainLast(t *testing.T, c *chain, chart string) int64 {
	t.Helper()
	last := jsonNumber(t, c.d("c"), "/api/v1/chart?chart="+chart, "last_entry")
	for _, n := range []string{"p", "gp"} {
		last = min(last, jsonNumber(t, c.d(n), "/host/parity-chain-child/api/v1/chart?chart="+chart, "last_entry"))
	}
	return last
}

// chainSeries is a chart's series over [after, before] at the child and as the proxy and the grandparent hold it.
func chainSeries(t *testing.T, c *chain, chart string, after, before int64) [3]string {
	t.Helper()
	return [3]string{
		seriesOf(t, c.d("c"), "", chart, after, before),
		seriesOf(t, c.d("p"), "/host/parity-chain-child", chart, after, before),
		seriesOf(t, c.d("gp"), "/host/parity-chain-child", chart, after, before),
	}
}

// csvRows are a CSV answer's header and rows (newest first; C ends each line with CRLF).
func csvRows(s string) (string, []string) {
	lines := strings.Split(strings.TrimRight(s, "\r\n"), "\r\n")
	if len(lines) == 0 {
		return "", nil
	}
	return lines[0], lines[1:]
}

// nullRow tells whether every value of a CSV row (after its time) is null.
func nullRow(row string) bool {
	f := strings.Split(row, ",")
	for _, v := range f[1:] {
		if v != "null" && v != "" {
			return false
		}
	}
	return len(f) > 1
}

// seriesExtraNulls compares a lower hop's series with an upper hop's: the same header and rows, except up to
// `allowed` rows the upper hop has all null where the lower has values (C's replication boundary, D105.10,
// D111.3). It returns the tolerated rows and, when they differ otherwise, the first difference.
func seriesExtraNulls(lower, upper string, allowed int) ([]string, string) {
	lh, lr := csvRows(lower)
	uh, ur := csvRows(upper)
	if lh != uh || len(lr) != len(ur) {
		return nil, firstDifference([]byte(lower), []byte(upper))
	}
	var tolerated []string
	for i := range lr {
		if lr[i] == ur[i] {
			continue
		}
		if nullRow(ur[i]) && !nullRow(lr[i]) && len(tolerated) < allowed {
			tolerated = append(tolerated, ur[i])
			continue
		}
		return tolerated, fmt.Sprintf("row %d: %q against %q", i, lr[i], ur[i])
	}
	return tolerated, ""
}

// chartFieldsRe is the chart a receiver's record names when its connection ends: the one its parser was reading, if
// any.
var chartFieldsRe = regexp.MustCompile(` instance=\S+ context=\S+`)

// chainRecords are a chain's streaming records: the child's, the proxy's RECEIVER LEFT and receiver records, and
// the grandparent's receiver records, each set sorted.
func chainRecords(t *testing.T, c *chain) map[string][]string {
	t.Helper()
	mask := func(lines []string) []string {
		for i, l := range lines {
			l = anyDstPortRe.ReplaceAllString(l, " dst_port=P")
			// without a chart the fields are empty, which leaves their spaces
			l = strings.Join(strings.Fields(chartFieldsRe.ReplaceAllString(l, "")), " ")
			lines[i] = mlCapableRe.ReplaceAllString(l, "ml_capable=M")
		}
		slices.Sort(lines)
		return slices.Compact(lines)
	}
	return map[string][]string{
		"child":                rchildRecords(t, c.d("c")),
		"proxy left":           mask(parentRecords(t, c.d("p"), "RECEIVER LEFT", nil)),
		"proxy receiver":       mask(parentRecords(t, c.d("p"), "STREAM RCV", nil)),
		"grandparent receiver": mask(parentRecords(t, c.d("gp"), "STREAM RCV", nil)),
	}
}

// startChains starts the three chains (`tweak`, when set, adjusting each one's rows), waits until each hop has its
// host online (the child starting once the proxy is online at the grandparent) and the child's own path has the
// three entries; a candidate chain that fails is marked down, the oracle's ends the test.
func startChains(t *testing.T, g *stagger, tweak func(rows []topoNode)) []*chain {
	bins := binaries(t)
	forms := []string{"c-c-c", "c-r-c", "r-c-r"}
	cs := make([]*chain, len(forms))
	errs := make([]error, len(forms))
	var wg sync.WaitGroup
	for i, form := range forms {
		// what a goroutine that ends early (a helper's Fatal) leaves: a chain that is down
		cs[i], errs[i] = &chain{form: form, down: true}, fmt.Errorf("%s did not start", form)
		wg.Add(1)
		go func() {
			defer wg.Done()
			rows := chainRows(form)
			if tweak != nil {
				tweak(rows)
			}
			tp, err := startTopology(t, g, "chain-"+form, bins, rows, func(tp *topology, stage int) error {
				if stage == 2 {
					if _, b, ok := waitHop(tp.addr("gp"), chainProxy.MachineGUID, true, 120*time.Second); !ok {
						return fmt.Errorf("%s: the grandparent never had the proxy online: %.300s", form, b)
					}
				}
				return nil
			})
			cs[i], errs[i] = &chain{form: form, topology: tp}, err
		}()
	}
	wg.Wait()
	for i, err := range errs {
		if err == nil {
			continue
		}
		if i == 0 {
			t.Fatalf("the oracle chain: %v", err)
		}
		t.Errorf("chain %s: %v", forms[i], err)
		cs[i].down = true
	}
	forChains(cs, func(c *chain) {
		launched := c.d("c").LaunchStartedAt
		for _, hop := range []struct{ at, guid string }{{"p", chainChild.MachineGUID}, {"gp", chainProxy.MachineGUID},
			{"gp", chainChild.MachineGUID}} {
			if _, b, ok := waitHop(c.addr(hop.at), hop.guid, true, 120*time.Second); !ok {
				t.Errorf("%s: %s never had %s online: %.300s", c.form, hop.at, hop.guid, b)
				c.down = true
				return
			}
			t.Logf("%s: %s has %s online %s after the child's launch", c.form, hop.at, hop.guid,
				time.Since(launched).Round(time.Second))
		}
		c.online = time.Now().Unix()
		if hops, ok := waitPathHops(c.addr("c"), "", fullPath, 30*time.Second); !ok {
			t.Errorf("%s: the child's own path has hops %v, not %v", c.form, hops, fullPath)
		}
	})
	if cs[0].down {
		t.FailNow()
	}
	return cs
}

// fullPath are the hops of the child's path through the whole chain.
var fullPath = []int{0, 1, 2}

// TestStreamChain (check `stream.chain`, milestone 7 commit 8i, D121.2, D122.1; plan
// `evidence/2026-09-30-plan-m7-8i-3-stream-chain.md`): three child → proxy → grandparent chains run side by side,
// c-c-c (the oracle everywhere), c-r-c (a Rust proxy) and r-c-r (a Rust child and grandparent); each candidate chain
// is compared with c-c-c, and within each chain the hops' data with each other. The child starts only once the
// proxy streams to the grandparent, so the proxy's `_is_parent` there is always false (it is sent once, at the
// proxy's sender's ready). Phases: online, the child's view, steady-state views, data, the child's leave, its return
// (the gap replicated through both hops), the records.
func TestStreamChain(t *testing.T) {
	g := &stagger{gap: 2 * time.Second}
	cs := startChains(t, g, nil)
	oracle := cs[0]
	full := fullPath
	for _, v := range []string{"gp", "p"} {
		if hops, ok := waitPathHops(oracle.addr(v), "&nodes=parity-chain-child", full, 10*time.Second); !ok {
			t.Fatalf("the oracle's %s path of the child has hops %v", v, hops)
		}
	}

	// P3: the steady state's views, compared with c-c-c, once the charts settle at the proxy and the grandparent
	// (a chart goes up at its next collection)
	settle := func(t *testing.T, c *chain) {
		for end := time.Now().Add(15 * time.Second); ; time.Sleep(time.Second) {
			p, gp := hopCharts(t, c.d("p"), false), hopCharts(t, c.d("gp"), false)
			if bytes.Equal(p, gp) {
				return
			}
			if time.Now().After(end) {
				t.Errorf("%s: the proxy's and the grandparent's charts of the child: %s", c.form, firstDifference(p, gp))
				return
			}
		}
	}
	var oracleCharts []byte
	var oracleInfo [2][]byte
	if !t.Run("c-c-c/steady", func(t *testing.T) {
		settle(t, oracle)
		oracleCharts = hopCharts(t, oracle.d("gp"), true)
		if len(oracleCharts) < 1000 {
			t.Fatalf("the grandparent's charts of the child: %.300s", oracleCharts)
		}
		for i, at := range []string{"gp", "p"} {
			b, err := streamInfoMasked(oracle.addr(at), chainChild.MachineGUID)
			if err != nil || !bytes.Contains(b, []byte(`"status":200`)) {
				t.Fatalf("%s stream_info of the child: %v %.300s", at, err, b)
			}
			oracleInfo[i] = b
		}
	}) {
		t.FailNow()
	}
	for _, c := range cs[1:] {
		if c.down {
			continue
		}
		t.Run(c.form+"/steady", func(t *testing.T) {
			chainViews(t, "steady", oracle, c, chainPathViews, entryTimes)
			for i, at := range []string{"gp", "p"} {
				b, err := streamInfoMasked(c.addr(at), chainChild.MachineGUID)
				if err != nil {
					t.Fatalf("%s stream_info of the child: %v", at, err)
				}
				if !bytes.Equal(oracleInfo[i], b) {
					t.Errorf("%s stream_info of the child: %s", at, firstDifference(oracleInfo[i], b))
				}
			}
			settle(t, c)
			if got := hopCharts(t, c.d("gp"), true); !bytes.Equal(oracleCharts, got) {
				t.Errorf("the grandparent's charts of the child: %s", firstDifference(oracleCharts, got))
			}
		})
	}

	// P4: the data within each chain, exact over the live window, tolerant from the child's first point
	online := oracle.online
	for _, c := range cs {
		if !c.down {
			online = max(online, c.online)
		}
	}
	time.Sleep(time.Until(time.Unix(online+30, 0)))
	for _, c := range cs {
		if c.down {
			continue
		}
		t.Run(c.form+"/data", func(t *testing.T) {
			for _, chart := range chainDataCharts {
				last := chainLast(t, c, chart) - 1
				live := chainSeries(t, c, chart, c.online+5, last)
				for j, pair := range [][2]int{{0, 1}, {1, 2}} {
					if live[pair[0]] != live[pair[1]] {
						t.Errorf("%s live, hop %d: %s", chart, j+1,
							firstDifference([]byte(live[pair[0]]), []byte(live[pair[1]])))
					}
				}
				if _, rows := csvRows(live[0]); len(rows) < 20 {
					t.Errorf("%s live: %d rows", chart, len(rows))
				}
				first := jsonNumber(t, c.d("c"), "/api/v1/chart?chart="+chart, "first_entry")
				whole := chainSeries(t, c, chart, first, last)
				for j, pair := range [][2]int{{0, 1}, {1, 2}} {
					tolerated, diff := seriesExtraNulls(whole[pair[0]], whole[pair[1]], 1)
					if diff != "" {
						t.Errorf("%s full, hop %d: %s", chart, j+1, diff)
					}
					if len(tolerated) > 0 {
						t.Logf("%s full, hop %d: tolerated %q", chart, j+1, tolerated)
					}
				}
			}
		})
	}

	// P5: the child leaves; the proxy and the grandparent keep its stale path
	forChains(cs, func(c *chain) {
		c.stopped = time.Now().Unix()
		if err := c.d("c").Stop(); err != nil {
			t.Errorf("%s: stop the child: %v", c.form, err)
		}
		if _, b, ok := waitHop(c.addr("gp"), chainChild.MachineGUID, false, 30*time.Second); !ok {
			t.Errorf("%s: the grandparent kept the child as a child: %.300s", c.form, b)
		}
	})
	time.Sleep(3 * time.Second)
	if len(parentRecords(t, oracle.d("p"), "RECEIVER LEFT", nil)) == 0 {
		t.Fatalf("the oracle proxy wrote no RECEIVER LEFT record")
	}
	for _, c := range cs[1:] {
		if !c.down {
			t.Run(c.form+"/leave", func(t *testing.T) {
				chainViews(t, "leave", oracle, c, [][2]string{gpChildView, proxyChildView}, entryTimes)
			})
		}
	}

	// P6: the child returns; its gap and the blocks the proxy lost at RECEIVER LEFT come through both hops
	forChains(cs, func(c *chain) {
		time.Sleep(time.Until(time.Unix(c.stopped+15, 0)))
		g.wait()
		c.returned = time.Now().Unix()
		if err := c.d("c").Restart(); err != nil {
			t.Errorf("%s: restart the child: %v", c.form, err)
			c.down = true
			return
		}
		if _, b, ok := waitHop(c.addr("gp"), chainChild.MachineGUID, true, 120*time.Second); !ok {
			t.Errorf("%s: the grandparent never had the returned child online: %.300s", c.form, b)
			c.down = true
			return
		}
		if hops, ok := waitPathHops(c.addr("c"), "", full, 30*time.Second); !ok {
			t.Errorf("%s: the returned child's own path has hops %v", c.form, hops)
		}
	})
	if oracle.down {
		t.FailNow()
	}
	for _, c := range cs {
		if c.down {
			continue
		}
		t.Run(c.form+"/return", func(t *testing.T) {
			for end := time.Now().Add(30 * time.Second); ; time.Sleep(time.Second) {
				settled := true
				for _, chart := range chainDataCharts {
					settled = settled && chainLast(t, c, chart) >= c.returned+10
				}
				if settled {
					break
				}
				if time.Now().After(end) {
					t.Errorf("the hops' data did not reach %d s after the return within 30 s", 10)
					break
				}
			}
			for _, chart := range chainDataCharts {
				s := chainSeries(t, c, chart, c.stopped-20, c.returned+10)
				if c == oracle {
					// newest first: values after the return, one run of nulls (the gap), values before the stop
					_, rows := csvRows(s[0])
					after, nulls, before, runs := 0, 0, 0, 0
					for k, r := range rows {
						switch {
						case nullRow(r):
							nulls++
							if k == 0 || !nullRow(rows[k-1]) {
								runs++
							}
						case nulls == 0:
							after++
						default:
							before++
						}
					}
					if after < 1 || nulls < 10 || runs != 1 || before < 15 {
						t.Errorf("%s: the oracle child's own series: %d rows after the return, %d null rows in %d runs, "+
							"%d rows before the stop", chart, after, nulls, runs, before)
					}
				}
				for j, pair := range [][2]int{{0, 1}, {1, 2}} {
					tolerated, diff := seriesExtraNulls(s[pair[0]], s[pair[1]], 1)
					if diff != "" {
						t.Errorf("%s, hop %d: %s", chart, j+1, diff)
					}
					if len(tolerated) > 0 {
						t.Logf("%s, hop %d: tolerated %q", chart, j+1, tolerated)
					}
				}
			}
			if c != oracle {
				chainViews(t, "return", oracle, c, chainPathViews, entryRestartTimes)
			}
		})
	}

	// P7: the records of both sessions and both leaves, as sets
	forChains(cs, func(c *chain) {
		if err := c.d("c").Stop(); err != nil {
			t.Errorf("%s: stop the child: %v", c.form, err)
		}
		if _, b, ok := waitHop(c.addr("gp"), chainChild.MachineGUID, false, 30*time.Second); !ok {
			t.Errorf("%s: the grandparent kept the child as a child: %.300s", c.form, b)
		}
	})
	time.Sleep(3 * time.Second)
	want := chainRecords(t, oracle)
	for _, name := range slices.Sorted(maps.Keys(want)) {
		t.Logf("oracle %s records:\n%s", name, strings.Join(want[name], "\n"))
	}
	for _, c := range cs[1:] {
		if c.down {
			continue
		}
		t.Run(c.form+"/records", func(t *testing.T) {
			got := chainRecords(t, c)
			for _, name := range slices.Sorted(maps.Keys(want)) {
				diffLines(t, name, want[name], got[name])
			}
		})
	}
}
