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

// pathEntry is one entry of a node's stream path in a /api/v3/stream_path answer.
type pathEntry struct {
	Hostname string `json:"hostname"`
	Hops     int    `json:"hops"`
	Start    int64  `json:"start_time"`
	Shutdown int64  `json:"shutdown_time"`
}

// pathEntries are each node's stream path in a /api/v3/stream_path answer.
func pathEntries(body []byte) ([][]pathEntry, error) {
	var v struct {
		Nodes []struct {
			Path []pathEntry `json:"streaming_path"`
		} `json:"nodes"`
	}
	if err := json.Unmarshal(body, &v); err != nil {
		return nil, err
	}
	out := make([][]pathEntry, len(v.Nodes))
	for i, n := range v.Nodes {
		out[i] = n.Path
	}
	return out, nil
}

// pathHops are the hops of each node's stream path in a /api/v3/stream_path answer.
func pathHops(body []byte) ([][]int, error) {
	nodes, err := pathEntries(body)
	if err != nil {
		return nil, err
	}
	out := make([][]int, len(nodes))
	for i, n := range nodes {
		for _, e := range n {
			out[i] = append(out[i], e.Hops)
		}
	}
	return out, nil
}

// entryTimesOf are the start and shutdown medians of a host's entry in a path answer.
func entryTimesOf(body []byte, hostname string) (start, shutdown int64, ok bool) {
	nodes, err := pathEntries(body)
	if err != nil {
		return 0, 0, false
	}
	for _, n := range nodes {
		for _, e := range n {
			if e.Hostname == hostname {
				return e.Start, e.Shutdown, true
			}
		}
	}
	return 0, 0, false
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

// downer is a topology's wrapper in a check, which a failure takes out of the comparisons.
type downer interface{ isDown() bool }

// forEach runs f on every wrapper that is up, in parallel; f may only report with t.Errorf.
func forEach[T downer](xs []T, f func(T)) {
	var wg sync.WaitGroup
	for _, x := range xs {
		if x.isDown() {
			continue
		}
		wg.Add(1)
		go func() {
			defer wg.Done()
			f(x)
		}()
	}
	wg.Wait()
}

// topoViews compares the stream path views of two topologies' agents: each view is a node name and a request;
// `_streams_to` and `maskLabels` are compared by presence only.
func topoViews(t *testing.T, stage string, oracle, cand *topology, views [][2]string, times *regexp.Regexp,
	maskLabels ...string) {
	t.Helper()
	for _, v := range views {
		compareStreamPathWith(t, stage+" "+v[0], [2]string{oracle.addr(v[0]), cand.addr(v[0])},
			[2]*strings.Replacer{}, v[1], times, append([]string{"_streams_to"}, maskLabels...)...)
	}
}

// entryRestartTimes also masks the timing medians a path entry carries, which a restart changes.
var entryRestartTimes = regexp.MustCompile(`("since":)\d+(,\s*"first_time_t":)\d+,\s*"start_time":\d+,\s*"shutdown_time":\d+`)

// hopCharts is an agent's /api/v1/charts of a host it holds, as parentCharts normalizes it, with `hosts` (every host
// on the agent) only when `withHosts`.
func hopCharts(t *testing.T, d *daemon.Daemon, host string, withHosts bool) []byte {
	t.Helper()
	b, err := rawExchange(d.Addr, []byte("GET /host/"+host+"/api/v1/charts HTTP/1.1\r\n\r\n"), 10*time.Second)
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

// hopLast is the newest point of a chart that every agent of `names` has of the host: names[0] is its origin, which
// holds it as its own.
func hopLast(t *testing.T, tp *topology, host string, names []string, chart string) int64 {
	t.Helper()
	last := jsonNumber(t, tp.d(names[0]), "/api/v1/chart?chart="+chart, "last_entry")
	for _, n := range names[1:] {
		last = min(last, jsonNumber(t, tp.d(n), "/host/"+host+"/api/v1/chart?chart="+chart, "last_entry"))
	}
	return last
}

// hopSeries is a chart's series over [after, before] at each agent of `names`: the origin's own, then as each holds
// the host.
func hopSeries(t *testing.T, tp *topology, host string, names []string, chart string, after, before int64) []string {
	t.Helper()
	out := []string{seriesOf(t, tp.d(names[0]), "", chart, after, before)}
	for _, n := range names[1:] {
		out = append(out, seriesOf(t, tp.d(n), "/host/"+host, chart, after, before))
	}
	return out
}

// waitHopsReach waits, up to `limit`, until every agent of `names` has each chart of the host up to `at`.
func waitHopsReach(t *testing.T, tp *topology, host string, names, charts []string, at int64, limit time.Duration) {
	t.Helper()
	for end := time.Now().Add(limit); ; time.Sleep(time.Second) {
		settled := true
		for _, chart := range charts {
			settled = settled && hopLast(t, tp, host, names, chart) >= at
		}
		if settled {
			return
		}
		if time.Now().After(end) {
			t.Errorf("the hops' data did not reach %d within %v", at, limit)
			return
		}
	}
}

// compareHops compares each hop's series of a chart with the next one's (consecutive agents of a path), tolerating
// one all-null row per hop (the replication boundary), which it logs.
func compareHops(t *testing.T, what string, series []string) {
	t.Helper()
	for j := 1; j < len(series); j++ {
		tolerated, diff := seriesExtraNulls(series[j-1], series[j], 1)
		if diff != "" {
			t.Errorf("%s, hop %d: %s", what, j, diff)
		}
		if len(tolerated) > 0 {
			t.Logf("%s, hop %d: tolerated %q", what, j, tolerated)
		}
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

// maskRecordSet masks what a streaming record carries by port or by timing (the destination port, the chart a
// disconnect names with the spaces its empty fields leave, ML's capability), as a sorted set.
func maskRecordSet(lines []string) []string {
	for i, l := range lines {
		l = anyDstPortRe.ReplaceAllString(l, " dst_port=P")
		l = strings.Join(strings.Fields(chartFieldsRe.ReplaceAllString(l, "")), " ")
		lines[i] = mlCapableRe.ReplaceAllString(l, "ml_capable=M")
	}
	slices.Sort(lines)
	return slices.Compact(lines)
}

// compareRecordSets compares a candidate chain's record sets with the oracle's.
func compareRecordSets(t *testing.T, want, got map[string][]string) {
	t.Helper()
	for _, name := range slices.Sorted(maps.Keys(want)) {
		diffLines(t, name, want[name], got[name])
	}
}

// relaunch boots a stopped or killed daemon again on its run directory and port, trying again while another
// process holds the port.
func relaunch(d *daemon.Daemon) error {
	var err error
	for range 5 {
		if err = d.Restart(); err == nil || !errors.Is(err, daemon.ErrPortTaken) {
			return err
		}
		time.Sleep(3 * time.Second)
	}
	return err
}
