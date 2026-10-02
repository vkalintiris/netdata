// SPDX-License-Identifier: GPL-3.0-or-later

// The `plugins.vnodes` subtests beyond the first draft (plan evidence/2026-10-01-plan-m8-commit4.md §4.2, designed by
// H5): claim-evicts, stale-never, scope and scope-dim, labels, redefine, restart-archived, three bad-input rows.

package parity

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/plugin"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

const (
	// vchildName is the child that streams the vnode's GUID before a plugin defines it (claim-evicts)
	vchildName = "parity-vchild"
	// vnodeRenamed is the vnode's name in redefine's second start
	vnodeRenamed = "parity-vnode-b"
)

// compareVnodeViewsAs is compareVnodeViews for the vnode under a given name, with its stream path too (the self
// entry's VIRTUAL and ephemeral flags and hops: strm.path.self, strm.path.flags).
func compareVnodeViewsAs(t *testing.T, p *Pair, stage, name string) {
	t.Helper()
	compareVnodeViewsTimes(t, p, stage, name, entryTimes)
}

// compareVnodeViewsTimes is compareVnodeViewsAs with the stream path's times masked by `times` (entryRestartTimes
// after a restart: each agent's own startup and shutdown medians).
func compareVnodeViewsTimes(t *testing.T, p *Pair, stage, name string, times *regexp.Regexp) {
	t.Helper()
	var hosts, info [2]string
	var charts [2][]string
	for i, side := range p.Each() {
		hosts[i] = archivedHostsView(t, side.Daemon)
		b, err := streamInfoMasked(side.Daemon.Addr, vnodeGUID)
		if err != nil {
			t.Fatal(err)
		}
		info[i] = string(b)
		ids, _, err := hostCharts(side.Daemon, name)
		if err != nil {
			t.Fatal(err)
		}
		charts[i] = ids
	}
	if !strings.Contains(hosts[0], name) {
		t.Errorf("%s: the oracle does not list the vnode %q: %s", stage, name, hosts[0])
	}
	if hosts[0] != hosts[1] {
		t.Errorf("%s: /api/v1/info hosts differ\noracle:    %s\ncandidate: %s", stage, hosts[0], hosts[1])
	}
	if info[0] != info[1] {
		t.Errorf("%s: the vnode's stream info differs\noracle:    %s\ncandidate: %s", stage, info[0], info[1])
	}
	diffLines(t, stage+": the vnode's charts", charts[0], charts[1])
	compareIdentity(t, p, stage, "/host/"+name+"/api/v1/info")
	compareStreamPath(t, stage+": the vnode's stream path", [2]string{p.Oracle.Addr, p.Candidate.Addr},
		"/api/v3/stream_path?nodes="+name, times)
}

// streamAnswers are each side's answers to a raw STREAM request for a host name and GUID.
func streamAnswers(t *testing.T, p *Pair, hostname, guid string) [2]string {
	t.Helper()
	var out [2]string
	for i, side := range p.Each() {
		q := fmt.Sprintf("key=%s&hostname=%s&machine_guid=%s&ver=17088", parentIdentity.StreamKey, hostname, guid)
		b, err := rawExchange(side.Daemon.Addr, streamRequest(q), 5*time.Second)
		if err != nil {
			t.Fatal(err)
		}
		out[i] = string(b)
	}
	return out
}

// checkRefused checks that both sides refuse a STREAM for the vnode's GUID as C refuses a locally collected vnode,
// with the same bytes (strm.hs.rsp.local_vnode, its fast path).
func checkRefused(t *testing.T, p *Pair, stage, hostname string) {
	t.Helper()
	answers := streamAnswers(t, p, hostname, vnodeGUID)
	if !strings.Contains(answers[0], "you are trying to stream my vnode back") {
		t.Errorf("%s: the oracle's answer: %q", stage, answers[0])
	}
	if answers[0] != answers[1] {
		t.Errorf("%s: the answers differ\noracle:    %q\ncandidate: %q", stage, answers[0], answers[1])
	}
}

// pollUntil polls ok every 250 ms until it holds or the timeout passes.
func pollUntil(timeout time.Duration, ok func() bool) bool {
	deadline := time.Now().Add(timeout)
	for !ok() {
		if time.Now().After(deadline) {
			return false
		}
		time.Sleep(250 * time.Millisecond)
	}
	return true
}

// streamInfoHas tells whether a daemon's stream_info of the vnode has a member, e.g. `"ingest_type":"virtual"`.
func streamInfoHas(addr, member string) bool {
	b, err := streamInfoMasked(addr, vnodeGUID)
	return err == nil && bytes.Contains(b, []byte(member))
}

// ingestIs tells whether a daemon's stream_info of the vnode reads this ingest type.
func ingestIs(addr, want string) bool {
	return streamInfoHas(addr, `"ingest_type":"`+want+`"`)
}

// waitStreamInfo waits until each side's stream_info of the vnode has a member. The oracle's timeout is fatal (the
// case did not run), the candidate's is reported and the comparisons that follow show how it differs.
func waitStreamInfo(t *testing.T, p *Pair, member string, timeout time.Duration) {
	t.Helper()
	for _, side := range p.Each() {
		if pollUntil(timeout, func() bool { return streamInfoHas(side.Daemon.Addr, member) }) {
			continue
		}
		b, _ := streamInfoMasked(side.Daemon.Addr, vnodeGUID)
		if side.Role == Oracle {
			t.Fatalf("oracle: the vnode's stream_info has no %s within %v: %s", member, timeout, b)
		}
		t.Errorf("candidate: the vnode's stream_info has no %s within %v: %s", member, timeout, b)
	}
}

// waitIngest waits until each side's stream_info of the vnode reads ingest type `want`: what proves the daemon
// processed the plugin's lines up to it.
func waitIngest(t *testing.T, p *Pair, want string, timeout time.Duration) {
	t.Helper()
	waitStreamInfo(t, p, `"ingest_type":"`+want+`"`, timeout)
}

// waitQueryable waits until each side's stream_info of the vnode reads db_status online. C reads `initializing`
// (and so ingest_status `initializing`, liveness `stale`) while the host's cached retention is zero
// (rrdhost-status.c:113-131, :171-177, :387-390); the context worker fills it when it post-processes the vnode's
// collected context, on its own 1 s heartbeat (rrdcontext-worker.c:975-981, rrdcontext-internal.h:16), so a view taken
// once the ingest type reads virtual races it. Only for a vnode with a stored point: C stores none at a chart's first
// collection (rrdset-collection.c:656-673), so after a single one it stays `initializing` (labels, exit).
func waitQueryable(t *testing.T, p *Pair, timeout time.Duration) {
	t.Helper()
	waitStreamInfo(t, p, `"db_status":"online"`, timeout)
}

// collectedSecs are the seconds a start's Collect steps wrote blocks for, in order.
func collectedSecs(start []plugin.Record) []int64 {
	var out []int64
	for _, r := range start {
		if r.Kind == "collected" {
			out = append(out, r.Sec)
		}
	}
	return out
}

// hasNoRecord reports a record containing text in any log class of the oracle.
func hasNoRecord(t *testing.T, classes map[string][]string, text string) {
	t.Helper()
	for class, lines := range classes {
		for _, l := range lines {
			if strings.Contains(l, text) {
				t.Errorf("oracle: %s has %q: %s", class, text, l)
			}
		}
	}
}

// vnodeChild streams the vnode's GUID into a daemon as the child vchildName: a host label, then a chart with three
// points; the connection stays open until the parent closes it or the test ends.
func vnodeChild(t *testing.T, d *daemon.Daemon) *stream.Conn {
	t.Helper()
	conn, err := stream.Connect(d.Addr, d.StreamKey, stream.HostInfo{Hostname: vchildName, MachineGUID: vnodeGUID}, stream.CapsLive)
	if err != nil {
		t.Fatalf("the vnode's child: %v", err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	conn.Linef(`LABEL "from" = 1 "child"`)
	conn.Linef("OVERWRITE labels")
	conn.Linef("CHART 'vchild.one' '' 'title' 'units' 'family' 'vchild.one' line 1000 1 '' fixture-pusher corpus")
	conn.Linef("DIMENSION 'd1' '' absolute 1 1 ''")
	now := time.Now().Unix()
	for i := int64(3); i >= 1; i-- {
		conn.Linef("BEGIN2 'vchild.one' 1 %d #", now-i)
		conn.Linef("SET2 'd1' %d %d A", i, i)
		conn.Linef("END2")
	}
	if err := conn.Flush(); err != nil {
		t.Fatalf("the vnode's child: %v", err)
	}
	return conn
}

// waitEvicted reads what the parent sends the child until the parent closes the connection; an error when it is
// still open at the timeout.
func waitEvicted(conn *stream.Conn, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	for {
		if _, err := conn.ReadLine(deadline); err != nil {
			if errors.Is(err, os.ErrDeadlineExceeded) {
				return fmt.Errorf("still connected after %v", timeout)
			}
			return nil
		}
	}
}

var (
	logMsgRe = regexp.MustCompile(`msg="((?:[^"\\]|\\.)*)"`)
	// the evicted receiver's record (stream-receiver.c:723-736): its stream thread's id, the peer's port and the
	// counters are each side's own
	rcvIDRe      = regexp.MustCompile(`STREAM RCV\[\d+\]`)
	rcvPeerRe    = regexp.MustCompile(`\[from \[[^\]]*\]:\d+\]`)
	rcvNumbersRe = regexp.MustCompile(`\b(msgs|bytes_in|bytes_out|connected|idle|repl)=\d+`)
	rcvReasonRe  = regexp.MustCompile(`reason="([^"]*)"`)
)

// evictionReasons are the reasons C may print for the evicted receiver: -40 reaches only rcv.status.reason; the
// record prints the remover's reason, SIGNALED TO STOP when the receiver sees its shutdown flag
// (stream-receiver.c:344, :856), SOCKET CLOSED BY REMOTE END when the poll sees the shut socket first (:1036-1044) [I]
var evictionReasons = []string{"DISCONNECTED SIGNALED TO STOP", "DISCONNECTED SOCKET CLOSED BY REMOTE END"}

// vnodeReceiverRecords are a daemon's `receiver disconnected` records of the child vchildName (the receiver thread's,
// not the plugin's), their messages masked (thread id, peer, counters, reason), and the reasons.
func vnodeReceiverRecords(t *testing.T, d *daemon.Daemon) (records, reasons []string) {
	t.Helper()
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		m := logMsgRe.FindStringSubmatch(l)
		if m == nil {
			continue
		}
		msg := strings.ReplaceAll(m[1], `\"`, `"`)
		if !strings.Contains(msg, "receiver disconnected") || !strings.Contains(msg, "'"+vchildName+"'") {
			continue
		}
		if r := rcvReasonRe.FindStringSubmatch(msg); r != nil {
			reasons = append(reasons, r[1])
		}
		msg = rcvIDRe.ReplaceAllString(msg, "STREAM RCV[N]")
		msg = rcvPeerRe.ReplaceAllString(msg, "[from [IP]:PORT]")
		msg = rcvNumbersRe.ReplaceAllString(msg, "$1=N")
		records = append(records, rcvReasonRe.ReplaceAllString(msg, `reason="R"`))
	}
	return records, reasons
}

// vnodeMetaRows are the rows naming the vnode in one database of a running daemon's cache (read from a copy, as
// dumpDB does), one table list per call; "" while the copy cannot be read (a write in flight).
func vnodeMetaRows(t *testing.T, d *daemon.Daemon, db string, tables ...string) string {
	t.Helper()
	dir, err := os.MkdirTemp(t.TempDir(), "meta-")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(dir)
	src := filepath.Join(d.Opts.RunDir, "cache", db)
	for _, suffix := range []string{"", "-wal", "-shm"} {
		if b, err := os.ReadFile(src + suffix); err == nil {
			_ = os.WriteFile(filepath.Join(dir, "db"+suffix), b, 0o644)
		}
	}
	args := []string{filepath.Join(dir, "db")}
	if db == "netdata-meta.db" {
		args = append(args, "--mask", "host.last_connected")
	}
	for _, table := range tables {
		args = append(args, "--table", table, "--sort", table)
	}
	out, err := exec.Command(metadataDump(t), args...).CombinedOutput()
	if err != nil {
		return ""
	}
	var rows []string
	for _, l := range strings.Split(string(out), "\n") {
		if strings.HasPrefix(l, "row ") && strings.Contains(l, hexID(vnodeGUID)) {
			rows = append(rows, l)
		}
	}
	return strings.Join(rows, "\n")
}

// waitVnodeStored waits until each side's metadata database holds the vnode: its host row, a host label row containing
// `label`, and its chart rows (the stop is not relied on to). C's metadata sync stores a host's row and labels, then
// its charts and their dimensions in one transaction, every 5 s (sqlite_metadata.c:2711-2722, :2302-2398, :2795-2797);
// an archived host's contexts load from those rows (rrdcontext-loading.c:180-188). An unclaimed C never writes
// context-meta.db's context rows: only rrdcontext_message_send_unsafe stores one (rrdcontext-worker.c:865-911), on a
// dispatch to the cloud (rrdcontext-queues.c:276-291) or a cloud checkpoint (rrdcontext.c:353).
func waitVnodeStored(t *testing.T, p *Pair, label string) {
	t.Helper()
	for _, side := range p.Each() {
		ok := pollUntil(30*time.Second, func() bool {
			return vnodeMetaRows(t, side.Daemon, "netdata-meta.db", "host") != "" &&
				strings.Contains(vnodeMetaRows(t, side.Daemon, "netdata-meta.db", "host_label"), label) &&
				vnodeMetaRows(t, side.Daemon, "netdata-meta.db", "chart") != ""
		})
		if !ok {
			if side.Role == Oracle {
				t.Fatalf("oracle: the vnode is not stored within 30 s")
			}
			t.Errorf("candidate: the vnode is not stored within 30 s")
		}
	}
}

// contextsLoaded tells whether a daemon answers a host's contexts with at least one (an archived host's load runs
// after the start); C prints the answer indented (api_v1_contexts.c:417, buffer.h:49-50).
func contextsLoaded(addr, name string) bool {
	b, err := rawExchange(addr, []byte("GET /host/"+name+"/api/v1/contexts HTTP/1.1\r\n\r\n"), 5*time.Second)
	if err != nil || !bytes.HasPrefix(b, []byte("HTTP/1.1 200 ")) {
		return false
	}
	var v struct {
		Contexts map[string]json.RawMessage `json:"contexts"`
	}
	return json.Unmarshal(httpBody(b), &v) == nil && len(v.Contexts) > 0
}

// contextTimesRe are a context's retention members in /api/v1/contexts.
var contextTimesRe = regexp.MustCompile(`"(first_time_t|last_time_t)":\d+`)

// compareArchivedVnode is compareArchived for the vnode with each side's own retention masked: each side collected
// it at its own seconds, where compareArchived's children come from one shared seed (its stream_info and contexts
// compare their times). Alternative: give compareArchivedTimes a retention mask. After the restart each stream path
// entry carries its agent's own startup and shutdown medians (stream-path.c:109-110): entryRestartTimes.
func compareArchivedVnode(t *testing.T, p *Pair) {
	t.Helper()
	compare := func(name string, get func(d *daemon.Daemon) string) {
		t.Helper()
		if o, c := get(p.Oracle), get(p.Candidate); o != c {
			t.Errorf("archived %s:\noracle:    %s\ncandidate: %s", name, o, c)
		}
	}
	compare("hosts", func(d *daemon.Daemon) string { return archivedHostsView(t, d) })
	compare("charts hosts", func(d *daemon.Daemon) string { return member(t, d, "/api/v1/charts", "hosts") })
	compare("info", func(d *daemon.Daemon) string {
		text, labels := infoIdentity(t, d.Addr, "/host/"+vnodeName+"/api/v1/info")
		return fmt.Sprintf("%s\n%v", text, labels) // fmt prints a map sorted by key
	})
	compare("contexts", func(d *daemon.Daemon) string {
		return contextTimesRe.ReplaceAllString(member(t, d, "/host/"+vnodeName+"/api/v1/contexts", "contexts"), `"$1":T`)
	})
	compare("stream_info", func(d *daemon.Daemon) string {
		b, err := streamInfoMasked(d.Addr, vnodeGUID)
		if err != nil {
			t.Fatal(err)
		}
		return string(b)
	})
	compareStreamPath(t, "archived", [2]string{p.Oracle.Addr, p.Candidate.Addr}, "/api/v3/stream_path?nodes="+vnodeName, entryRestartTimes)
	compare("records", func(d *daemon.Daemon) string {
		var out []string
		for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
			if threadOf(l) == "" && archivedRecords.MatchString(l) {
				out = append(out, normalizeLog(l, d.Opts.RunDir, ""))
			}
		}
		return strings.Join(out, "\n")
	})
}

// releaseAll releases a WaitFile step on each side.
func releaseAll(t *testing.T, ls [2]plugin.Layout, name string) {
	t.Helper()
	for i := range ls {
		if err := ls[i].Release(name); err != nil {
			t.Fatal(err)
		}
	}
}

// waiting holds once start n (from 1) waits for the release file.
func waiting(n int, file string) func([][]plugin.Record) bool {
	return func(s [][]plugin.Record) bool { return len(s) >= n && plugin.Has(s[n-1], "waiting", file) }
}

// moreVnodeCases are the planned subtests the first draft of TestPluginsVnodes lacks.
func moreVnodeCases() map[string]pluginCase {
	hang := plugin.Step{Hang: true}
	define := plugin.Step{Emit: vnodeDefine(vnodeGUID, vnodeName, vnodeLabels...)}
	pd := "daemon.log thread=PD[difftest]"
	return map[string]pluginCase{
		// a child streams the vnode's GUID (ram, as stream.conf's key says), then the plugin defines it: the host is
		// updated (renamed, its program and memory mode), claimed and the receiver evicted; the child's reconnect is
		// refused (C pluginsd_parser.c:228-255, :297-310; rrdhost.c:695-832; stream-receiver.c:1518-1557)
		"claim-evicts": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				{WaitFile: "release-1"}, define,
				{Collect: &plugin.Collect{Chart: "difftest.vc", Dims: []string{"x"}, N: 2}},
				{WaitFile: "release-2"}, hang,
			}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not start", waiting(1, "release-1"))
				var conns [2]*stream.Conn
				for i, side := range p.Each() {
					conns[i] = vnodeChild(t, side.Daemon)
				}
				for _, side := range p.Each() {
					if !pollUntil(30*time.Second, func() bool { ok, _ := hasReceiver(side.Daemon.Addr, vnodeGUID); return ok }) {
						_, b := hasReceiver(side.Daemon.Addr, vnodeGUID)
						t.Fatalf("%s: no receiver for the vnode's GUID: %s", side.Role, b)
					}
				}
				releaseAll(t, ls, "release-1")
				for i, side := range p.Each() {
					if err := waitEvicted(conns[i], 20*time.Second); err != nil {
						if side.Role == Oracle {
							t.Fatalf("oracle: the child was not evicted: %v", err)
						}
						t.Errorf("candidate: the child was not evicted: %v", err)
					}
				}
				waitPlugin(t, p, ls, "the plugin did not collect", waiting(1, "release-2"))
				waitIngest(t, p, "virtual", 10*time.Second)
				waitQueryable(t, p, 10*time.Second)
				compareVnodeViewsAs(t, p, "claimed", vnodeName)
				// the child's reconnect: refused at the fast path, the vnode keeps its name
				checkRefused(t, p, "the child's reconnect", vchildName)
				var records, reasons [2][]string
				for i, side := range p.Each() {
					records[i], reasons[i] = vnodeReceiverRecords(t, side.Daemon)
					for _, r := range reasons[i] {
						if !slices.Contains(evictionReasons, r) {
							t.Errorf("%s: the evicted receiver's reason %q is none of C's %q", side.Role, r, evictionReasons)
						}
					}
				}
				if len(records[0]) != 1 {
					t.Errorf("oracle: %d records of the evicted receiver: %q", len(records[0]), records[0])
				}
				t.Logf("the evicted receiver's reasons: oracle %q, candidate %q", reasons[0], reasons[1])
				diffLines(t, "the evicted receiver's records", records[0], records[1])
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				hasRecord(t, classes, pd, fmt.Sprintf("Host '%s' has been renamed to '%s'. If this is not intentional it may mean multiple hosts are using the same machine_guid.", vchildName, vnodeName))
				hasRecord(t, classes, pd, fmt.Sprintf("Host '%s' switched program name from 'query-corpus-pusher' to 'netdata'", vnodeName))
				hasRecord(t, classes, pd, fmt.Sprintf("Host '%s' has memory mode 'ram', but the wanted one is 'dbengine'. Restart netdata here to apply the new settings.", vnodeName))
				hasRecord(t, classes, pd, fmt.Sprintf("PLUGINSD: HOST_DEFINE_END: host '%s' (machine guid %s) was receiving a stream while it is collected locally as a vnode - the stream has been disconnected. If this is a real child node, its machine guid conflicts with the guid of a locally collected vnode.", vnodeName, vnodeGUID))
				hasRecord(t, classes, pd, fmt.Sprintf(`VNODE: Configuring node stale after 0 seconds for host \"%s\"`, vnodeName))
			},
		},
		// the vnode's labels say `_node_stale_after_seconds 2`; C zeroes it before use (pluginsd_parser.c:136, :355),
		// so eight seconds of `HOST localhost` (each a stale walk opportunity, :363-404) leave it virtual: an agent that
		// honoured the 2 s would clear VIRTUAL|COLLECTOR_ONLINE by the second walk (D145.1)
		"stale-never": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: slices.Concat(
				[]plugin.Step{define, {Collect: &plugin.Collect{Chart: "difftest.vs", Dims: []string{"x"}, N: 1}}},
				staleTicks(8),
				[]plugin.Step{{Collect: &plugin.Collect{Chart: "difftest.ls", Dims: []string{"x"}, N: 2}}, {WaitFile: "release-1"}, hang},
			)}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				starts := waitPlugin(t, p, ls, "the plugin did not tick and wait", waiting(1, "release-1"))
				// the localhost chart's last block is after every HOST localhost: once a side shows it, its parser has
				// read them all
				for i, side := range p.Each() {
					secs := collectedSecs(starts[i][0])
					last := secs[len(secs)-1]
					if !pollUntil(10*time.Second, func() bool {
						_, entries, err := hostCharts(side.Daemon, "")
						return err == nil && entries["difftest.ls"].LastEntry >= last-1
					}) {
						t.Errorf("%s: localhost's difftest.ls did not reach %d", side.Role, last)
					}
				}
				if since := time.Since(plugin.Time(starts[0][0], "step")); since < 8*time.Second {
					t.Fatalf("oracle: only %v since the define (the window is 8 s)", since)
				}
				if !ingestIs(p.Oracle.Addr, "virtual") {
					t.Errorf("oracle: the vnode is not virtual after the window")
				}
				compareVnodeViewsAs(t, p, "after the window", vnodeName)
				checkRefused(t, p, "after the window", vnodeName)
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				hasRecord(t, classes, pd, fmt.Sprintf(`VNODE: Configuring node stale after 0 seconds for host \"%s\"`, vnodeName))
				hasNoRecord(t, classes, "as STALE")
			},
		},
		// C keeps the scope chart across HOST (pluginsd_parser.c:414) and END completes it on its own host (:104-124):
		// a block BEGIN localhost-chart / HOST g / SET / END / HOST localhost collects into localhost's chart; then
		// HOST_DEFINE_END inside a BEGIN clears the scope chart (:338-339), so the SET after it disables the plugin
		"scope": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				define, {Emit: "HOST localhost\n"},
				{Collect: &plugin.Collect{Chart: "difftest.sc", Dims: []string{"x"}, N: 4,
					InBlock: "HOST " + vnodeGUID, AfterBlock: "HOST localhost"}},
				{WaitFile: "release-1"},
				{Emit: "BEGIN difftest.sc\n" + vnodeDefine(vnodeGUID, vnodeName, vnodeLabels...) + "SET x = 9\n"},
				hang,
			}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				starts := waitPlugin(t, p, ls, "the plugin did not collect", waiting(1, "release-1"))
				var points, vcharts [2][]string
				for i, side := range p.Each() {
					secs := collectedSecs(starts[i][0])
					pollUntil(10*time.Second, func() bool {
						points[i] = pluginPoints(t, side.Daemon, "difftest.sc", secs[0], secs[len(secs)-1])
						return len(points[i]) >= len(secs)-1
					})
					ids, _, err := hostCharts(side.Daemon, vnodeName)
					if err != nil {
						t.Fatal(err)
					}
					vcharts[i] = ids
				}
				if len(points[0]) < 2 || slices.Contains(vcharts[0], "difftest.sc") {
					t.Errorf("oracle: localhost's points %v, the vnode's charts %v", points[0], vcharts[0])
				}
				diffLines(t, "localhost's difftest.sc points", points[0], points[1])
				diffLines(t, "the vnode's charts", vcharts[0], vcharts[1])
				releaseAll(t, ls, "release-1")
				waitPlugin(t, p, ls, "the plugin did not end", startEnded(1))
				// a disabled plugin is not started again (an enabled one would be after one update every)
				time.Sleep(3 * time.Second)
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 1 {
					t.Errorf("oracle: %d starts", len(starts))
				}
				hasRecord(t, classes, pd, "PLUGINSD: command SET requires a chart defined via command CHART, but is not set.")
				hasRecord(t, classes, pd, "parser_action('SET') failed on line")
			},
		},
		// under HOST g a SET's records name the scope host while the chart is localhost's [C!]
		// (pluginsd_internals.h:262-353): an unknown dimension disables the plugin
		"scope-dim": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				define, {Emit: "HOST localhost\n"},
				{Collect: &plugin.Collect{Chart: "difftest.sd", Dims: []string{"x"}, N: 1}},
				{Emit: "BEGIN difftest.sd\nHOST " + vnodeGUID + "\nSET zz = 1\n"}, hang,
			}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not end", startEnded(1))
				time.Sleep(3 * time.Second)
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 1 {
					t.Errorf("oracle: %d starts", len(starts))
				}
				hasRecord(t, classes, pd, fmt.Sprintf("PLUGINSD: 'host:%s/chart:difftest.sd/dim:zz' got a SET but dimension does not exist.", vnodeName))
			},
		},
		// labels into system info and the host's labels (pluginsd_parser.c:170-186, :316-332;
		// rrdhost-system-info.c:204-252): `_os` windows in any case, `_is_ephemeral` normalised (On is true;
		// inicfg_api.c:250), `_collector_machine_guid` replaced by localhost's, an empty value kept as `[none]`, a name
		// sanitized to empty refused with an ERR (rrdlabels.c:353-355), another sanitized
		"labels": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				{Emit: vnodeDefine(vnodeGUID, vnodeName,
					"_os", "WINDOWS", "_os_name", "Ubuntu", "_os_version", "22.04", "_kernel_version", "6.1.0",
					"_system_cores", "4", "_system_ram_total", "1073741824", "_architecture", "x86_64",
					"_virtualization", "kvm", "_container", "none", "_is_k8s_node", "false", "_hw_sys_vendor", "Acme",
					"_net_default_iface", "eth0", "_cloud_provider_type", "aws", "_is_ephemeral", "On",
					"_collector_machine_guid", "00000000-0000-4000-8000-000000000000",
					"role", "", "@@", "x", "Bad Name!", "v,1")},
				{Collect: &plugin.Collect{Chart: "difftest.vl", Dims: []string{"x"}, N: 1}},
				{WaitFile: "release-1"}, hang,
			}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not collect", waiting(1, "release-1"))
				waitIngest(t, p, "virtual", 10*time.Second)
				compareVnodeViewsAs(t, p, "defined", vnodeName)
				text, labels := infoIdentity(t, p.Oracle.Addr, "/host/"+vnodeName+"/api/v1/info")
				want := map[string]any{"_is_ephemeral": "true", "_collector_machine_guid": parentIdentity.MachineGUID,
					"role": "[none]", "Bad_Name_": "v.1"}
				for k, v := range want {
					if labels[k] != v {
						t.Errorf("oracle: label %s = %v, want %v", k, labels[k], v)
					}
				}
				if _, ok := labels["@@"]; ok || !strings.Contains(text, `"os_name":"Microsoft Windows"`) {
					t.Errorf("oracle: %s %v", text, labels)
				}
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				// netdata_log_error(): an errno field; C vs C decides whether it needs a mask [I]
				hasRecord(t, classes, pd, "rrdlabels_add_changed: cannot add name '@@' (value 'x') which is sanitized as empty string")
			},
		},
		// the next start defines the GUID with another name and labels: the host is updated (rename WARNING,
		// rrdhost.c:740-746), its labels migrated (rrdlabels.c:732; a dropped label goes), `_is_ephemeral 1` reads
		// false (only yes/true/on/auto/on demand are true)
		"redefine": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{
					{Emit: vnodeDefine(vnodeGUID, vnodeName, "role", "web", "zone", "a", "_is_ephemeral", "yes", "_os_name", "Linux")},
					{Collect: &plugin.Collect{Chart: "difftest.vr", Dims: []string{"x"}, N: 2}},
					{WaitFile: "release-1"}, {Exit: plugin.ExitCode(0)}}},
				{Steps: []plugin.Step{
					{Emit: vnodeDefine(vnodeGUID, vnodeRenamed, "role", "db", "tier", "1", "_is_ephemeral", "1", "_os_name", "FreeBSD")},
					{Collect: &plugin.Collect{Chart: "difftest.vr", Dims: []string{"x"}, N: 2}},
					{WaitFile: "release-2"}, hang}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not collect", waiting(1, "release-1"))
				waitIngest(t, p, "virtual", 10*time.Second)
				waitQueryable(t, p, 10*time.Second)
				compareVnodeViewsAs(t, p, "first", vnodeName)
				releaseAll(t, ls, "release-1")
				starts := waitPlugin(t, p, ls, "no second start", waiting(2, "release-2"))
				checkRestart(t, starts, 2, time.Second, 2500*time.Millisecond, "one update every after a run with a collection")
				for _, side := range p.Each() {
					if !pollUntil(10*time.Second, func() bool { return strings.Contains(archivedHostsView(t, side.Daemon), vnodeRenamed) }) {
						t.Errorf("%s: the vnode is not renamed", side.Role)
					}
				}
				waitIngest(t, p, "virtual", 10*time.Second)
				waitQueryable(t, p, 10*time.Second)
				compareVnodeViewsAs(t, p, "redefined", vnodeRenamed)
				_, labels := infoIdentity(t, p.Oracle.Addr, "/host/"+vnodeRenamed+"/api/v1/info")
				if _, ok := labels["zone"]; ok || labels["role"] != "db" || labels["_is_ephemeral"] != "false" {
					t.Errorf("oracle: the migrated labels: %v", labels)
				}
				// SQLite: the renamed host's rows, once each side stored them (host, host_info, host_label sorted)
				var rows [2]string
				for i, side := range p.Each() {
					pollUntil(30*time.Second, func() bool {
						rows[i] = vnodeMetaRows(t, side.Daemon, "netdata-meta.db", "host", "host_info", "host_label")
						return strings.Contains(rows[i], vnodeRenamed) && strings.Contains(rows[i], "tier")
					})
				}
				if !strings.Contains(rows[0], vnodeRenamed) || rows[0] != rows[1] {
					t.Errorf("the vnode's rows differ\noracle:\n%s\ncandidate:\n%s", rows[0], rows[1])
				}
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 2 {
					t.Errorf("oracle: %d starts", len(starts))
				}
				hasRecord(t, classes, pd, "PLUGINSD: Reseting virtual host status for "+vnodeName)
				hasRecord(t, classes, pd, fmt.Sprintf("Host '%s' has been renamed to '%s'. If this is not intentional it may mean multiple hosts are using the same machine_guid.", vnodeName, vnodeRenamed))
				hasRecord(t, classes, pd, fmt.Sprintf(`VNODE: Configuring node stale after 0 seconds for host \"%s\"`, vnodeRenamed))
			},
		},
		// define, store, let go (exit 0; `update every = 30` holds the next start past the restart, so no plugin
		// process is alive at the restart and no start's records race it), restart both on their caches: the vnode
		// reloads archived (sqlite_aclk.c:1248-1300, hops > 0); then the next start defines it again (rrdhost_update
		// revives it, rrdhost.c:792-826)
		"restart-archived": {
			ue: 30,
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{define, {Collect: &plugin.Collect{Chart: "difftest.va", Dims: []string{"x"}, N: 3}},
					{WaitFile: "release-1"}, {Exit: plugin.ExitCode(0)}}},
				{Steps: []plugin.Step{{WaitFile: "release-2"}, define,
					{Collect: &plugin.Collect{Chart: "difftest.va", Dims: []string{"x"}, N: 2}}, {WaitFile: "release-3"}, hang}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not collect", waiting(1, "release-1"))
				waitIngest(t, p, "virtual", 10*time.Second)
				waitVnodeStored(t, p, "role")
				compareVnodeViewsAs(t, p, "defined", vnodeName)
				releaseAll(t, ls, "release-1")
				waitPlugin(t, p, ls, "the first start did not end", startEnded(1))
				restartBoth(t, p)
				for _, side := range p.Each() {
					if !pollUntil(30*time.Second, func() bool { return contextsLoaded(side.Daemon.Addr, vnodeName) }) {
						t.Errorf("%s: the archived vnode's contexts did not load", side.Role)
					}
				}
				if !ingestIs(p.Oracle.Addr, "archived") {
					t.Errorf("oracle: the vnode is not archived after the restart")
				}
				if !slices.ContainsFunc(logLines(t, p.Oracle.Opts.RunDir, "daemon.log"), func(l string) bool {
					return strings.Contains(l, "Created 1 archived hosts (0 children and 1 vnodes)")
				}) {
					t.Errorf("oracle: no record of the archived vnode's load")
				}
				compareArchivedVnode(t, p)
				// the revival: the second start (begun at the restart) defines it again
				waitPlugin(t, p, ls, "no second start", waiting(2, "release-2"))
				releaseAll(t, ls, "release-2")
				waitPlugin(t, p, ls, "the second start did not collect", waiting(2, "release-3"))
				waitIngest(t, p, "virtual", 10*time.Second)
				waitQueryable(t, p, 10*time.Second)
				compareVnodeViewsTimes(t, p, "revived", vnodeName, entryRestartTimes)
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 2 {
					t.Errorf("oracle: %d starts", len(starts))
				}
				hasRecord(t, classes, pd, "PLUGINSD: Reseting virtual host status for "+vnodeName)
			},
			// the restart's stop finds the plugin thread in its sleep after the first start (plugins_d.c:62-65): whether
			// it sees the collectors stop and ends before the cleanup's check, which then does not name it, is a race
			// (plugins_d.c:9-17, :186-190, :216-217); the final stop's record (the second start hangs) is compared
			mask: func(classes map[string][]string) {
				class := "daemon.log thread=PLUGINSD"
				lines := classes[class]
				end := slices.IndexFunc(lines, func(l string) bool { return strings.Contains(l, "PLUGINSD: cleanup completed.") })
				if end < 0 {
					return
				}
				first := slices.DeleteFunc(slices.Clone(lines[:end]), func(l string) bool {
					return strings.Contains(l, "stopping plugin thread: plugin:"+plugin.Name)
				})
				classes[class] = append(first, lines[end:]...)
			},
		},
		// the plan's bad-input rows the first draft lacks (one pair each: the D limiter is process-wide)
		"host-bad-guid":  badVnodeCase("HOST zz\n", "PLUGINSD: keyword HOST: cannot parse MACHINE_GUID - is it a valid UUID?"),
		"define-no-name": badVnodeCase("HOST_DEFINE "+vnodeGUID+" ''\n", "PLUGINSD: keyword HOST_DEFINE: missing parameters"),
		"label-no-value": badVnodeCase("HOST_DEFINE "+vnodeGUID+" "+vnodeName+"\nHOST_LABEL k\n", "PLUGINSD: keyword HOST_LABEL: missing parameters"),
	}
}

// staleTicks are n seconds of `HOST localhost`, one a second: each runs C's stale walk when its next check is due.
func staleTicks(n int) []plugin.Step {
	var out []plugin.Step
	for range n {
		out = append(out, plugin.Step{Emit: "HOST localhost\n"}, plugin.Step{SleepMs: 1000})
	}
	return out
}
