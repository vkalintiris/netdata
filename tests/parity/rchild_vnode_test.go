// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/url"
	"os"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/plugin"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The vnode the children's fake plugin defines (the plan's g2 `rchild-v`), and its chart: one dimension counting
// from 1, a block a second.
const (
	rvGUID  = "5a1e0000-0000-4000-8000-00000000c0b2"
	rvName  = "rchild-v"
	rvChart = "difftest.rv"
)

// rvLabels define the vnode: system info its handshake carries (rrdhost-system-info.c:204-252, :721-752) and a label
// of its own.
var rvLabels = []string{"_os_name", "Linux", "_os_version", "12", "_kernel_version", "6.1.0", "_architecture", "x86_64",
	"_system_cores", "4", "_system_ram_total", "1073741824", "_virtualization", "kvm", "_container", "none",
	"_is_k8s_node", "false", "_cloud_provider_type", "aws", "_cloud_instance_type", "t3.micro",
	"_cloud_instance_region", "eu-west-1", "role", "edge"}

// rvDefine is the plugin's first step; from HOST_DEFINE_END on its lines go to the vnode (pluginsd_parser.c:338).
var rvDefine = plugin.Step{Emit: vnodeDefine(rvGUID, rvName, rvLabels...)}

// rvCollect collects the vnode's chart for n seconds.
func rvCollect(n int) plugin.Step {
	return plugin.Step{Collect: &plugin.Collect{Chart: rvChart, Dims: []string{"x"}, N: n}}
}

// rvTo streams to a parent with the parents' API key and the 5 s reconnect floor: a vnode's sender starts at its
// first collection and connects 5-10 s later (command-begin-set-end-init.c:21-26; stream-connector.c:510,
// stream-parents.c:113-130).
func rvTo(destination string) *daemon.StreamTo {
	return &daemon.StreamTo{Destination: destination, APIKey: parentIdentity.StreamKey, Extra: "    reconnect delay = 5\n"}
}

// rvChild starts a child with `stream.rchild`'s identity that runs the fake plugin playing `sc` and streams as `to`
// says: pluginsOptions with the agent's own charts on (the pulse trap, R61-4: a C parent calls the child online only
// once it holds its data), alloc, one tier, no API key section; `logs` is a [logs] section. It returns its errors:
// runBoth calls it off the test's goroutine (R64-12).
func rvChild(t *testing.T, bin string, role Role, to *daemon.StreamTo, logs string, sc plugin.Scenario,
	adjust ...func(*daemon.Options)) (*daemon.Daemon, plugin.Layout, error) {
	t.Helper()
	engine, err := plugin.Engine()
	if err != nil {
		return nil, plugin.Layout{}, err
	}
	o := pluginsOptions(1, nil, nil, logs)
	o.PulseOff = false
	id := daemon.Identity{Hostname: rchildHostname, StreamKey: cChildKey, MachineGUID: rchildGUID}
	o.Binary, o.RunDir, o.Identity = bin, runDir(t, role), &id
	o.DBMode, o.NoStreamKey, o.StreamTo = "alloc", true, to
	for _, f := range adjust {
		f(&o)
	}
	l, err := plugin.Install(o.RunDir, engine, sc)
	if err != nil {
		return nil, l, err
	}
	d, err := daemon.Start(o)
	if err != nil {
		return nil, l, fmt.Errorf("parity: start %s: %w", role, err)
	}
	t.Cleanup(func() { _ = d.Stop() })
	return d, l, nil
}

// rvSetup starts two C parents (`ram`, one tier) and a C child and a Rust child playing `sc`, child i streaming to
// parent i.
func rvSetup(t *testing.T, sc plugin.Scenario) (*Pair, [2]*daemon.Daemon, [2]plugin.Layout) {
	t.Helper()
	bins := binaries(t)
	p := startPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1}, parentIdentity,
		[2]string{bins[0], bins[0]}, [2]string{}, [2]Role{"rcv-parent-oracle", "rcv-parent-candidate"})
	var children [2]*daemon.Daemon
	var ls [2]plugin.Layout
	for i, side := range p.Each() {
		var err error
		children[i], ls[i], err = rvChild(t, bins[i], Role("rcv-child-"+strconv.Itoa(i)), rvTo(side.Daemon.Addr), "", sc)
		if err != nil {
			t.Fatal(err)
		}
	}
	return p, children, ls
}

// rvConnected waits until each parent holds both of its child's connections (a parent's receivers are its total,
// stream-parents.c:336) and calls the vnode and the child online, then 5 s for the sessions' metadata. The C child's
// side failing is fatal (the case did not run); the Rust child's is reported and the comparisons show how it
// differs. It logs each side's time from the vnode's first collection.
func rvConnected(t *testing.T, p *Pair, ls [2]plugin.Layout) {
	t.Helper()
	for i, side := range p.Each() {
		addr := side.Daemon.Addr
		ok := pollUntil(60*time.Second, func() bool { ok, _ := hasReceivers(addr, rvGUID, 2); return ok }) &&
			ingestOnline(addr, rvGUID, 60*time.Second) && ingestOnline(addr, rchildGUID, 60*time.Second)
		if !ok {
			_, b := hasReceivers(addr, rvGUID, 2)
			if i == 0 {
				t.Fatalf("%s: the vnode and the child are not both online: %s", side.Role, httpBody(b))
			}
			t.Errorf("%s: the vnode and the child are not both online: %s", side.Role, httpBody(b))
			continue
		}
		if starts, err := ls[i].Starts(); err == nil && len(starts) > 0 {
			t.Logf("%s: the vnode online %v after its first collection", side.Role,
				time.Since(plugin.Time(starts[0], "collected")).Round(time.Second))
		}
	}
	time.Sleep(5 * time.Second)
}

// rvPath is the vnode's stream path in a daemon's /api/v3/stream_path?nodes=<vnode> (its one node's).
func rvPath(t *testing.T, addr string) []streamPathEntry {
	t.Helper()
	b, err := rawExchange(addr, []byte("GET /api/v3/stream_path?nodes="+rvName+" HTTP/1.1\r\n\r\n"), 10*time.Second)
	if err != nil {
		t.Errorf("%s: the vnode's stream path: %v", addr, err)
		return nil
	}
	var v struct {
		Nodes []struct {
			Path []streamPathEntry `json:"streaming_path"`
		} `json:"nodes"`
	}
	if err := json.Unmarshal(httpBody(b), &v); err != nil || len(v.Nodes) != 1 {
		t.Errorf("%s: the vnode's stream path: %v: %s", addr, err, httpBody(b))
		return nil
	}
	return v.Nodes[0].Path
}

// checkRvEntry reports unless the path holds an entry of `guid` with these hops, flagged virtual or not.
func checkRvEntry(t *testing.T, stage string, path []streamPathEntry, guid string, hops int, virtual bool) {
	t.Helper()
	for _, e := range path {
		if e.HostID == guid {
			if e.Hops != hops || slices.Contains(e.Flags, "virtual") != virtual {
				t.Errorf("oracle: %s: the vnode's path entry of %s: %+v, want hops %d, virtual %t", stage, guid, e, hops, virtual)
			}
			return
		}
	}
	t.Errorf("oracle: %s: the vnode's path has no entry of %s: %+v", stage, guid, path)
}

// rvNodesRules compare the C parents' /api/v2/nodes of the vnode (D145.10: the endpoint is M10's): the request's
// durations masked, the labels a set (C prints them in heap order).
var rvNodesRules = Rules{
	Masks:     []Mask{{Pattern: "timings", Reason: "request durations"}},
	Unordered: []string{"nodes.[].labels"},
	Settle:    10 * time.Second,
}

// rvCompareParents compares what the two C parents hold of their children's vnode while it streams: its path on
// each parent and each child (the child's entry VIRTUAL, hops 0, and the parent's, hops 2: stream-path.c:100-145,
// stream-connector.c:137), its stream_info, its charts, the C parents' /api/v2/nodes of it, and within each side the
// parent's series of its chart against the child's own.
func rvCompareParents(t *testing.T, p *Pair, children [2]*daemon.Daemon) {
	t.Helper()
	parents := [2]string{p.Oracle.Addr, p.Candidate.Addr}
	compareStreamPath(t, "parents", parents, "/api/v3/stream_path?nodes="+rvName, entryTimes, "_streams_to")
	compareStreamPath(t, "children", [2]string{children[0].Addr, children[1].Addr},
		"/api/v3/stream_path?nodes="+rvName, entryTimes, "_streams_to")
	path := rvPath(t, p.Oracle.Addr)
	checkRvEntry(t, "parent", path, rchildGUID, 0, true)
	checkRvEntry(t, "parent", path, parentIdentity.MachineGUID, 2, false)
	var info, defs [2][]byte
	for i, addr := range parents {
		b, err := streamInfoMasked(addr, rvGUID)
		if err != nil {
			t.Fatal(err)
		}
		info[i] = b
		b, err = rawExchange(addr, []byte("GET /host/"+rvName+"/api/v1/charts HTTP/1.1\r\n\r\n"), 5*time.Second)
		if err != nil {
			t.Fatal(err)
		}
		defs[i] = rvCharts(t, httpBody(b))
	}
	if !bytes.Contains(info[0], []byte(`"status":200`)) || !bytes.Contains(info[0], []byte(`"ingest_type":"child"`)) {
		t.Errorf("oracle: the parent's stream_info of the vnode: %s", info[0])
	}
	if !bytes.Equal(info[0], info[1]) {
		t.Errorf("the vnode's stream_info differs:\noracle:    %s\ncandidate: %s", info[0], info[1])
	}
	if !bytes.Contains(defs[0], []byte(`"`+rvChart+`"`)) || !bytes.Contains(defs[0], []byte(`"Netdata Virtual Host 1.0"`)) {
		t.Errorf("oracle: the parent's charts of the vnode lack its chart or its os:\n%s", defs[0])
	}
	if !bytes.Equal(defs[0], defs[1]) {
		t.Errorf("the parents' charts of the vnode differ:\noracle:\n%s\ncandidate:\n%s", defs[0], defs[1])
	}
	diffs, err := p.CompareJSON("/api/v2/nodes", url.Values{"scope_nodes": {rvName}}, rvNodesRules)
	if err != nil {
		t.Fatal(err)
	}
	for _, d := range diffs {
		t.Errorf("/api/v2/nodes of the vnode: %s", d)
	}
	if r, err := Get(p.Oracle, "/api/v2/nodes", url.Values{"scope_nodes": {rvName}}); err != nil ||
		!bytes.Contains(r.Body, []byte(`"`+rvGUID+`"`)) {
		t.Errorf("oracle: /api/v2/nodes does not list the vnode: %v: %s", err, r.Body)
	}
	// within each side, the parent's series of the vnode's chart equal the child's own over settled seconds
	time.Sleep(2 * time.Second)
	now := time.Now().Unix()
	for i, side := range p.Each() {
		own := chartCSV(t, children[i].Addr, "/host/"+rvName, rvChart, now-12, now-2)
		streamed := chartCSV(t, side.Daemon.Addr, "/host/"+rvName, rvChart, now-12, now-2)
		if rows := strings.Count(own, "\n"); rows < 8 {
			t.Errorf("%s: the child's own series of the vnode has %d rows: %q", side.Role, rows, own)
		} else if own != streamed {
			t.Errorf("%s: the parent's series of the vnode differ from the child's own:\nchild:  %s\nparent: %s",
				side.Role, own, streamed)
		}
	}
}

// rvChildRecords are a child's streaming records (rchildRecordsWith) and its plugin thread's records naming the vnode,
// in their text or their `node=` field (its creation with its sender's destination, rrdhost.c:646-678; its run's end,
// pluginsd_parser.c:1519-1521; the parser's records while the vnode is the scope host), or streaming (the vnode
// sender's gate, command-begin-set-end-init.c:21-45), masked by rvMaskRecords. The plugin thread's
// are cut at the child's `stop`: the stop's cancel races the plugin's EOF exit, so "PARSER: thread cancelled while
// waiting for data." comes in some C runs only (R64-1d); the stream threads' stop records stay (the shutdown hand-back,
// stable as in `stream.rchild`).
func rvChildRecords(t *testing.T, d *daemon.Daemon, stop time.Time) []string {
	t.Helper()
	return rvMaskRecords(rchildRecordsWith(t, d, func(th, l string) bool {
		return th == "PD["+plugin.Name+"]" && (strings.Contains(l, "STREAM ") || strings.Contains(l, rvName)) &&
			loggedBefore(l, stop)
	}))
}

// loggedBefore tells whether a record's time (`time=`, milliseconds) is before `stop`.
func loggedBefore(l string, stop time.Time) bool {
	m := recordTimeRe.FindStringSubmatch(l)
	if m == nil {
		return false
	}
	at, err := time.Parse(time.RFC3339Nano, m[1])
	return err == nil && at.Before(stop)
}

// rvStreamInfoCountsRe are a parent's host and receiver counts in a child's record of that parent's stream_info answer
// ("STREAM PARENTS '<host>': failed to extract fields from JSON stream info response …", the JSON quoted): they are
// the C parent's, and depend on which of the child's two hosts probes first, each 5-10 s after its start
// (stream-parents.c:113-130; rv1 `exit-idle`, rv2 `parents`: 1/0 and 2/1 swapped between the hosts).
var rvStreamInfoCountsRe = regexp.MustCompile(`(\\"(?:nodes|receivers)\\":)\d+`)

// rvMaskRecords masks what differs between two runs of one agent in a child's normalized records, then dedups and
// sorts them: a block's wall-clock second in the gate records' `request="'END' '<sec>' '0'"` (fnEndTimeRe; rv1:
// 1790906534 against …537), the stream API key the Rust agent masks (D31, D34), the parent's counts in a stream_info
// record (rvStreamInfoCountsRe).
func rvMaskRecords(lines []string) []string {
	var out []string
	for _, l := range lines {
		l = fnEndTimeRe.ReplaceAllString(l, `request="'END' 'T'`)
		l = hsHostKeyRe.ReplaceAllString(l, "with api key 'K'")
		if strings.Contains(l, "stream info response") {
			l = rvStreamInfoCountsRe.ReplaceAllString(l, "${1}N")
		}
		if !slices.Contains(out, l) {
			out = append(out, l)
		}
	}
	slices.Sort(out)
	return out
}

// rvCharts is parentCharts with its `hosts` sorted by name: a parent lists its hosts in creation order, and which of
// the child and its vnode connects first is random (R64-1c; rv2 `parents`: `rchild-v` and `parity-rchild` swapped).
func rvCharts(t *testing.T, body []byte) []byte {
	t.Helper()
	var v map[string]any
	if err := json.Unmarshal(parentCharts(t, body), &v); err != nil {
		t.Fatalf("charts: %v", err)
	}
	hosts, _ := v["hosts"].([]any)
	// fmt prints a map sorted by key
	slices.SortFunc(hosts, func(a, b any) int { return strings.Compare(fmt.Sprint(a), fmt.Sprint(b)) })
	out, _ := json.MarshalIndent(v, "", " ")
	return out
}

// idleCloseRe are the figures of a receiver's idle close (stream-receiver.c:1213-1219): what it sent and how long the
// connection idled
var idleCloseRe = regexp.MustCompile(`sent \d+ bytes in \d+ operations, it is idle for [^,]+,`)

// rvParentRecords are a parent's records naming the vnode (its host's creation, its receivers' and their ends), each
// once, normalized as parentRecords does, with the receivers' counters (rcvNumbersRe) and idle closes masked too.
func rvParentRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	seen := map[string]bool{}
	var out []string
	for _, l := range parentRecords(t, d, "'"+rvName+"'", nil) {
		l = rcvNumbersRe.ReplaceAllString(l, "$1=N")
		l = idleCloseRe.ReplaceAllString(l, "sent N bytes in N operations, it is idle for D,")
		if !seen[l] {
			seen[l] = true
			out = append(out, l)
		}
	}
	slices.Sort(out)
	return out
}

// rvCompareRecords stops the children and compares their records, then the parents' records of the vnode, as sets,
// and checks that neither parent's parser rejected anything of its child's streams. `want` are texts the C child's
// records must hold.
func rvCompareRecords(t *testing.T, p *Pair, children [2]*daemon.Daemon, want ...string) {
	t.Helper()
	var records, rcv [2][]string
	for i, c := range children {
		stop := time.Now()
		_ = c.Stop()
		records[i] = rvChildRecords(t, c, stop)
	}
	for _, w := range want {
		if !slices.ContainsFunc(records[0], func(l string) bool { return strings.Contains(l, w) }) {
			t.Errorf("oracle: the C child's records lack %q", w)
		}
	}
	diffLines(t, "children's records", records[0], records[1])
	// the parents log their receivers' ends
	time.Sleep(2 * time.Second)
	for i, side := range p.Each() {
		rcv[i] = rvParentRecords(t, side.Daemon)
		// a C parent's parser logs PLUGINSD records of a stream only for input it rejects or tolerates with an error
		// (its INFO ones are a plugin run's, pluginsd_parser.c:1457, :1519-1521): none for either child (R64-11; the
		// map's "no PLUGINSD parse error", R42 m5.6). On the C child's side this is the guard.
		if parsed := pluginsdRecords(t, side.Daemon); len(parsed) > 0 {
			t.Errorf("%s: the parent's parser complained of its child's streams:\n%s", side.Role, strings.Join(parsed, "\n"))
		}
	}
	if len(rcv[0]) == 0 {
		t.Errorf("oracle: the parent has no records of the vnode")
	}
	diffLines(t, "the parents' records of the vnode", rcv[0], rcv[1])
	t.Logf("children's records:\n%s\nthe parent's records of the vnode:\n%s", strings.Join(records[0], "\n"),
		strings.Join(rcv[0], "\n"))
}

// rvRefusing answers the vnode's STREAM requests as a C parent collecting the vnode itself does
// (stream-receiver-connection.c:624-653) and accepts the rest in plaintext.
func rvRefusing(p *stream.Parent) {
	p.SetScript(func(r stream.Request) stream.Answer {
		if r.Params.Get("machine_guid") == rvGUID {
			return stream.Answer{Reply: stream.RejectLocalVnode, Close: true}
		}
		return stream.PlaintextAnswer(r)
	})
}

// recordAt is when a daemon first logged a record holding every text, zero when none.
func recordAt(t *testing.T, d *daemon.Daemon, texts ...string) time.Time {
	t.Helper()
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if !slices.ContainsFunc(texts, func(s string) bool { return !strings.Contains(l, s) }) {
			if m := recordTimeRe.FindStringSubmatch(l); m != nil {
				at, _ := time.Parse(time.RFC3339Nano, m[1])
				return at
			}
		}
	}
	return time.Time{}
}

// TestRChildVnode (check `stream.rchild-vnode`, M8 commit 4, D145.12; plan evidence/2026-10-01-plan-m8-commit4.md
// §4.3): a C child and a Rust child each run the fake plugin, which defines the vnode rvName and collects its chart;
// each child streams the vnode upstream on a connection of its own (strm.snd.per_host).
//   - parents: each child streams to its own C parent. Compared: the vnode's stream path on the parents and the
//     children, its stream_info and charts on the parents, the C parents' /api/v2/nodes of it, within each side the
//     parent's series of its chart against the child's own, and after the stop the children's records and the
//     parents' records of the vnode, as sets.
//   - capture: each child streams to a recording stub. Compared: the vnode's session (its request line raw, its
//     parameters and headers, its start: CLAIMED_ID, labels as a set, the first stream path's payload; its chart and
//     data shapes) and the vnode's sender gate records.
//   - refused: the stub answers the vnode as a C parent collecting it answers (and accepts localhost); compared: the
//     connector's records at debug level (the local-vnode row: DEBUG, 3600 s, its postpone checked and masked).
//   - exit-idle (PARITY_LONG, ~12 min): the plugin's run ends; the vnode's sender stays connected and silent until
//     each C parent's 600 s idle close, then connects again.
//   - retention-change (PARITY_LONG, ~5 min): an alloc child keeping 60 points; a vnode chart freed after going
//     obsolete arms C's full retention pass, which raises the vnode's first time: its session gets a second path.
func TestRChildVnode(t *testing.T) {
	t.Run("parents", func(t *testing.T) {
		p, children, ls := rvSetup(t, plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
			rvDefine, rvCollect(120), {Hang: true}}}}})
		rvConnected(t, p, ls)
		rvCompareParents(t, p, children)
		rvCompareRecords(t, p, children,
			"Host '"+rvName+"' (at registry as '"+rvName+"') with guid '"+rvGUID+"' initialized",
			"STREAM SND '"+rvName+"': streaming is not ready", "STREAM SND '"+rvName+"': streaming is ready")
	})
	t.Run("capture", func(t *testing.T) {
		sc := plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{rvDefine, rvCollect(90), {Hang: true}}}}}
		var caps [2]capture
		var lines, paths, gates [2][]string
		var vers [2]uint32
		runBoth(t, func(i int, bin string, role Role) {
			stub, err := stream.StartParent(nil)
			if err != nil {
				t.Error(err)
				return
			}
			t.Cleanup(func() { stub.Close() })
			d, _, err := rvChild(t, bin, role+"-vn", rvTo(stub.Addr()), "", sc)
			if err != nil {
				t.Error(err)
				return
			}
			s := stub.WaitSessionFor(rvGUID, 1, 60*time.Second)
			if s == nil {
				t.Errorf("%s: no STREAM connection for the vnode within 60 s (%d sessions)", role, len(stub.Sessions()))
				return
			}
			if !s.WaitData(func(b []byte) bool { return bytes.Contains(b, []byte("\nEND2\n")) }, 30*time.Second) {
				t.Errorf("%s: no data of the vnode within 30 s", role)
			}
			time.Sleep(10 * time.Second)
			if len(stub.SessionsFor(rchildGUID)) == 0 {
				t.Errorf("%s: localhost has no session of its own", role)
			}
			_ = d.Stop()
			caps[i] = parseCapture(s.Request, s.Data())
			lines[i], vers[i] = []string{s.Request.Line}, s.Request.Caps()
			// the path comes at ready; whether the vnode's first retention is known by then or follows as a second
			// path is timing (rrdcontext-worker.c:83-107), so the start keeps the first (retention-change counts them)
			kept := false
			caps[i].start = slices.DeleteFunc(caps[i].start, func(l string) bool {
				drop := kept && l == "JSON STREAM_PATH <payload>"
				kept = kept || l == "JSON STREAM_PATH <payload>"
				return drop
			})
			// C sends 1 path or 2, the second with the vnode's first retention (R64-9)
			blocks := streamPathBlocks(s.Data())
			if len(blocks) > 2 {
				t.Errorf("%s: %d stream paths in the vnode's session, C sends at most 2", role, len(blocks))
			} else if len(blocks) == 2 {
				var v struct {
					Path []streamPathEntry `json:"streaming_path"`
				}
				if err := json.Unmarshal(blocks[1], &v); err != nil || len(v.Path) != 1 || v.Path[0].First == 0 {
					t.Errorf("%s: the vnode's second stream path has no first time: %v: %s", role, err, blocks[1])
				}
			}
			if len(blocks) > 0 {
				paths[i] = []string{string(entryTimes.ReplaceAll(blocks[0], []byte("${1}0${2}0")))}
			}
			t.Logf("%s: the vnode's session: %d bytes, %d stream paths", role, len(s.Data()), len(blocks))
			for _, r := range senderRecords(t, d) {
				if strings.Contains(r, "'"+rvName+"'") {
					gates[i] = append(gates[i], r)
				}
			}
		})
		if t.Failed() {
			return
		}
		// the C child's vnode as C sends it (stream-connector.c:137, :164-189; rrdhost-system-info.c:721-752;
		// command-claimed_id.c:54-74; stream-sender.c:138-142, :165-172; pluginsd_parser.c:322-325)
		for _, w := range []string{"os=Netdata Virtual Host 1.0", "hops=2", "hostname=" + rvName,
			"machine_guid=" + rvGUID, "NETDATA_SYSTEM_OS_NAME=Linux", "NETDATA_INSTANCE_CLOUD_TYPE=aws",
			"User-Agent: netdata/"} {
			if !slices.ContainsFunc(caps[0].request, func(l string) bool { return strings.HasPrefix(l, w) }) {
				t.Errorf("oracle: the vnode's request lacks %q: %q", w, caps[0].request)
			}
		}
		if !strings.Contains(lines[0][0], "&ml_capable=0&ml_enabled=0&mc_version=0&") || vers[0]&stream.CapMLModels != 0 {
			t.Errorf("oracle: the vnode's request: %s", lines[0][0])
		}
		for _, re := range []string{`^CLAIMED_ID '` + rvGUID + `' 'NULL'$`,
			`^LABEL "_collector_machine_guid" = \d+ "` + rchildGUID + `"$`, `^LABEL "_net_default_iface" = \d+ "lo"$`,
			`^LABEL "role" = \d+ "edge"$`} {
			if !slices.ContainsFunc(caps[0].start, regexp.MustCompile(re).MatchString) {
				t.Errorf("oracle: the vnode's session start lacks %s:\n%s", re, strings.Join(caps[0].start, "\n"))
			}
		}
		if len(paths[0]) == 0 || !strings.Contains(paths[0][0], `"host_id":"`+rchildGUID+`"`) ||
			!strings.Contains(paths[0][0], `"hops":0`) || !strings.Contains(paths[0][0], `"virtual"`) {
			t.Errorf("oracle: the vnode's first stream path: %q", paths[0])
		}
		if len(caps[0].data[rvChart]) == 0 {
			t.Errorf("oracle: no data of %s in the vnode's session", rvChart)
		}
		compareCaptures(t, "the vnode's session", caps[0], caps[1])
		diffLines(t, "the vnode's request lines", lines[0], lines[1])
		diffLines(t, "the vnode's first stream paths", paths[0], paths[1])
		diffLines(t, "the vnode's gate records", gates[0], gates[1])
		t.Logf("the vnode's request: %s\nits start:\n%s\nits gate:\n%s", lines[0][0], strings.Join(caps[0].start, "\n"),
			strings.Join(gates[0], "\n"))
	})
	t.Run("refused", func(t *testing.T) {
		sc := plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{rvDefine, rvCollect(60), {Hang: true}}}}}
		var records [2][]string
		runBoth(t, func(i int, bin string, role Role) {
			s := startStubs(t, map[string]func(*stream.Parent){"P-vnode": rvRefusing})
			d, _, err := rvChild(t, bin, role+"-vn", rvTo(s.destination()), "    level = debug\n", sc)
			if err != nil {
				t.Error(err)
				return
			}
			waitRecords(t, d, s, map[string]string{"P-vnode": "the vnode is collected locally on that server"},
				60*time.Second)
			// localhost's first connect is random too (5-10 s after its start): the stop waits for its gate's ready
			// record, so localhost's connector and gate records are whole on both sides (R64-1e)
			if !pollUntil(30*time.Second, func() bool {
				return !recordAt(t, d, "thread=PULSE", "STREAM SND '"+rchildHostname+"': streaming is ready").IsZero()
			}) {
				t.Errorf("%s: localhost's sender not ready within 30 s", role)
			}
			time.Sleep(2 * time.Second)
			// C postpones the parent randomize(5, 3600) s: a second try within the case is possible, and harmless (the
			// records are a set)
			t.Logf("%s: %d sessions of the vnode, %d of localhost", role,
				len(s.parents["P-vnode"].SessionsFor(rvGUID)), len(s.parents["P-vnode"].SessionsFor(rchildGUID)))
			_ = d.Stop()
			records[i] = rvMaskRecords(handshakeRecordsWith(t, d, s, "PULSE", "PD["+plugin.Name+"]"))
		})
		want := fmt.Sprintf("STREAM CONNECT '%s' [to 127.0.0.1:P-vnode]: remote server rejected this stream, the vnode "+
			"is collected locally on that server - will retry in 3600 secs, at T", rvName)
		if !slices.ContainsFunc(records[0], func(l string) bool { return strings.Contains(l, want) }) {
			t.Errorf("oracle: no record %q", want)
		}
		diffLines(t, "records", records[0], records[1])
		t.Logf("records:\n%s", strings.Join(records[0], "\n"))
	})
	t.Run("exit-idle", func(t *testing.T) {
		if os.Getenv("PARITY_LONG") != "1" {
			t.Skip("PARITY_LONG unset (about twelve minutes: C's 600 s receiver idle timeout)")
		}
		// the run collects 12 s, waits for the release, exits; the next start defines nothing (C starts it again one
		// update every after a run with data, plugins_d.c:61-65)
		p, children, ls := rvSetup(t, plugin.Scenario{Starts: []plugin.Start{
			{Steps: []plugin.Step{rvDefine, rvCollect(12), {WaitFile: "release-1"}, {Exit: plugin.ExitCode(0)}}},
			{Steps: []plugin.Step{{Hang: true}}},
		}})
		rvConnected(t, p, ls)
		var last [2]time.Time
		for i, side := range p.Each() {
			starts, ok := ls[i].WaitFor(30*time.Second, waiting(1, "release-1"))
			if !ok {
				t.Fatalf("%s: the vnode's collection did not end", side.Role)
			}
			secs := collectedSecs(starts[0])
			last[i] = time.Unix(secs[len(secs)-1], 0)
			if err := ls[i].Release("release-1"); err != nil {
				t.Fatal(err)
			}
		}
		// the run's end clears VIRTUAL and COLLECTOR_ONLINE (pluginsd_parser.c:1508-1530) and leaves the sender
		// connected: every metadata push needs COLLECTOR_ONLINE (rrdhost.h:149-153), the connector does not ask
		// (stream-connector.c:491-517), nothing is collected
		for i, c := range children {
			if !pollUntil(15*time.Second, func() bool {
				return !recordAt(t, c, "PLUGINSD: Reseting virtual host status for "+rvName).IsZero()
			}) {
				t.Errorf("%s: no run end for the vnode", []Role{Oracle, Candidate}[i])
			}
		}
		time.Sleep(5 * time.Second)
		for i, side := range p.Each() {
			if ok, b := hasReceivers(side.Daemon.Addr, rvGUID, 2); !ok {
				if i == 0 {
					t.Fatalf("%s: the vnode's receiver went with the run's end: %s", side.Role, httpBody(b))
				}
				t.Errorf("%s: the vnode's receiver went with the run's end: %s", side.Role, httpBody(b))
			}
		}
		// the children's view of the vnode they let go: their entry without VIRTUAL, hops -1 (stream-path.c:130-136)
		compareStreamPath(t, "children after the run's end", [2]string{children[0].Addr, children[1].Addr},
			"/api/v3/stream_path?nodes="+rvName, entryTimes, "_streams_to")
		checkRvEntry(t, "child after the run's end", rvPath(t, children[0].Addr), rchildGUID, -1, false)
		// each parent closes the silent connection 600 s after its last traffic (stream-receiver.c:13, :434-438,
		// :1186-1221), then the child connects again (reconnect delay 5 s)
		closeTexts := []string{"'" + rvName + "'", "there was not traffic for 600 seconds - closing connection"}
		var closed [2]time.Time
		pollUntil(time.Until(slices.MaxFunc(last[:], time.Time.Compare).Add(660*time.Second)), func() bool {
			for i, side := range p.Each() {
				if closed[i].IsZero() {
					closed[i] = recordAt(t, side.Daemon, closeTexts...)
				}
			}
			return !closed[0].IsZero() && !closed[1].IsZero()
		})
		for i, side := range p.Each() {
			if closed[i].IsZero() {
				t.Errorf("%s: the parent did not close the vnode's idle connection within 660 s", side.Role)
				continue
			}
			idle := closed[i].Sub(last[i])
			t.Logf("%s: the parent closed the vnode's connection %v after its last collection", side.Role, idle.Round(time.Second))
			if idle < 599*time.Second || idle > 615*time.Second {
				t.Errorf("%s: the parent closed the vnode's connection %v after its last collection, C's is 600 s", side.Role, idle)
			}
			if !pollUntil(60*time.Second, func() bool { ok, _ := hasReceivers(side.Daemon.Addr, rvGUID, 2); return ok }) {
				t.Errorf("%s: the vnode did not connect again within 60 s of the close", side.Role)
			}
		}
		time.Sleep(5 * time.Second)
		rvCompareRecords(t, p, children, "PLUGINSD: Reseting virtual host status for "+rvName)
	})
	t.Run("retention-change", func(t *testing.T) {
		if os.Getenv("PARITY_LONG") != "1" {
			t.Skip("PARITY_LONG unset (about five minutes: C's full retention pass comes 120 s after a chart's free)")
		}
		obsolete := "CHART difftest.rg '' 'difftest' 'units' 'family' '' line 1000 1 'obsolete' '' ''\n"
		sc := plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{rvDefine,
			{Collect: &plugin.Collect{Chart: "difftest.rg", Dims: []string{"x"}, N: 3}}, {WaitFile: "release-1"},
			{Emit: obsolete}, rvCollect(300), {Hang: true}}}}}
		alloc60 := func(o *daemon.Options) { o.DBExtra = "    retention = 60\n    cleanup obsolete charts after = 10s\n" }
		runBoth(t, func(i int, bin string, role Role) {
			stub, err := stream.StartParent(nil)
			if err != nil {
				t.Error(err)
				return
			}
			t.Cleanup(func() { stub.Close() })
			d, l, err := rvChild(t, bin, role+"-vn", rvTo(stub.Addr()), "", sc, alloc60)
			if err != nil {
				t.Error(err)
				return
			}
			s := stub.WaitSessionFor(rvGUID, 1, 60*time.Second)
			if s == nil {
				t.Errorf("%s: no STREAM connection for the vnode within 60 s", role)
				return
			}
			if !s.WaitData(func(b []byte) bool { return bytes.Contains(b, []byte("OVERWRITE labels\n")) }, 30*time.Second) {
				t.Errorf("%s: no host labels of the vnode within 30 s", role)
				return
			}
			// the path the change is measured from is the last one with a first time: the vnode's first retention may
			// come after its ready path, as a path of its own (rrdcontext-worker.c:83-107)
			from := -1
			found := pollUntil(20*time.Second, func() bool {
				blocks := streamPathBlocks(s.Data())
				var v struct {
					Path []streamPathEntry `json:"streaming_path"`
				}
				if len(blocks) == 0 || json.Unmarshal(blocks[len(blocks)-1], &v) != nil || len(v.Path) == 0 {
					return false
				}
				from = len(blocks) - 1
				return v.Path[0].First != 0
			})
			if !found {
				t.Errorf("%s: no stream path of the vnode with a first time within 20 s of its labels (%d paths)", role,
					from+1)
				return
			}
			if err := l.Release("release-1"); err != nil {
				t.Error(err)
				return
			}
			first, second, ok := retentionPaths(t, string(role), s, rchildGUID, time.Now(), from)
			if ok && (!slices.Contains(first.Flags, "virtual") || !slices.Contains(second.Flags, "virtual")) {
				t.Errorf("%s: the vnode's paths are not flagged virtual: %v, %v", role, first.Flags, second.Flags)
			}
			_ = d.Stop()
		})
	})
}
