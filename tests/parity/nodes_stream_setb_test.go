// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"math"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/fixture"
	"github.com/netdata/netdata/tests/query-corpus/plugin"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// Set B of the node instances' and netdata-streaming's rows (milestone 10 commits 11 and 12, D241 F3, D248; the plan's
// section 4.2): `/api/v3/node_instances` and the admin's netdata-streaming table in the states set A leaves out. The
// pairs: a parent whose fixture child replicates (setbRepl), two proxies whose proxied child replicates up to one
// scripted parent (setbRelay), a virtual node beside a localhost that collects (setbVnode), an archived dbengine host
// with its retention (setbArchived), a vnode's claim of a child's host and its release (setbClaim); and, asked from
// fnStreamCatalog, fn.stream's real C child with its vnode on a receiver of its own (fnStreamNodeInstances).

// setbFormat is C's JSON text of a double (buffer_print_netdata_double, libnetdata/buffer/buffer.h:682-697, through
// print_netdata_double, :491-550): `null` for a NaN or an infinity; else the integral part and, when it is not 0, the
// fraction rounded to seven digits (llrint, to even) without its trailing zeros. Only for a value below 1.8e18, which
// every completion is (C writes a larger one with an exponent, :502-512).
func setbFormat(v float64) string {
	if math.IsNaN(v) || math.IsInf(v, 0) {
		return "null"
	}
	sign := ""
	if v < 0 {
		sign, v = "-", -v
	}
	whole, frac := math.Modf(v)
	integral, fraction := uint64(whole), uint64(math.RoundToEven(frac*1e7))
	if fraction >= 1e7 {
		integral, fraction = integral+1, fraction-1e7
	}
	text := sign + strconv.FormatUint(integral, 10)
	if fraction != 0 {
		text += "." + strings.TrimRight(fmt.Sprintf("%07d", fraction), "0")
	}
	return text
}

// setbCompletion is C's text of a replication's completion: the seconds from started to current of the window from
// started to now, in percent, not clamped (a receiver's, taken at a replication answer that does not start streaming:
// plugins.d/pluginsd_replication.c:420-428; a sender's, taken at each status: database/rrdhost-status.c:81-97; DEFECTS,
// commit 11), written as C writes a double (setbFormat).
func setbCompletion(started, current, now int64) string {
	return setbFormat(float64(current-started) * 100 / float64(now-started))
}

// setbCompletionRe is a replication's completion in a node-instance answer (api_v2_contexts.c:357, :397): a number
// as C writes a double, or null.
var setbCompletionRe = regexp.MustCompile(`"completion":(\s*)(-?[0-9][0-9.e+]*|null)`)

// setbRow finds the row of the table whose Node is name (-1: none).
func setbRow(v Value, name string) int {
	data, _ := dashMember(v, "data")
	for r := range data.Items {
		if c, err := fnStreamingCell(v, r, "Node"); err == nil && c.Text == name {
			return r
		}
	}
	return -1
}

// setbCells are row r's cells of the columns names, each a whole number (a null fails).
func setbCells(v Value, r int, names ...string) (map[string]int64, error) {
	out := map[string]int64{}
	for _, name := range names {
		c, err := fnStreamingCell(v, r, name)
		if err != nil {
			return nil, err
		}
		n, null, err := fnStreamingWhole(c)
		if err != nil || null {
			return nil, fmt.Errorf("row %d: %s is %s, want a whole number", r, name, c)
		}
		out[name] = n
	}
	return out, nil
}

// setbWithin holds when the milliseconds ms are of a whole second of the window w.
func setbWithin(ms int64, w [2]int64) bool {
	return ms%1000 == 0 && ms/1000 >= w[0] && ms/1000 <= w[1]
}

// ---------------------------------------------------------------------------------------------------------------
// replicating: a child whose second chart replicates (setbRepl)

// setbReplHost is the replicating child.
var setbReplHost = stream.HostInfo{Hostname: "parity-repl", MachineGUID: "5a1e0000-0000-4000-8000-00000000b501"}

// The replicating child's two charts: the one it replicates to its end, the one it leaves open.
const (
	setbReplDone = "repl.done"
	setbReplOpen = "repl.open"
)

// setbReplSide is what the replicating rows take from one side's own run: niSide's windows (opened: the child's
// connection, at the return its second one) and the receiver's replication: the oldest `after` the parent asked on
// this connection (started: the receiver's replication.first_time_s, streaming/stream-replication-receiver.c:75-76),
// the end of the open chart's partial answer (current: the parser's replay end time, pluginsd_replication.c:422) and
// the seconds that answer's REND was taken in (answered: from before the child wrote it to after the parent's next
// request for the chart was read, which C sends from the same REND, pluginsd_replication.c:596-600).
type setbReplSide struct {
	niSide
	started, current int64
	answered         [2]int64
}

// partials are the texts C writes for the open chart's partial answer on the side: setbCompletion of the answer at
// each second of answered (none before it).
func (s setbReplSide) partials() []string {
	var out []string
	if s.answered[1] == 0 {
		return nil
	}
	for now := s.answered[0]; now <= s.answered[1]; now++ {
		out = append(out, setbCompletion(s.started, s.current, now))
	}
	return out
}

// setbPartialRender writes PARTIAL for each completion of body that is one of partials.
func setbPartialRender(partials []string, body []byte) []byte {
	return setbCompletionRe.ReplaceAllFunc(body, func(m []byte) []byte {
		g := setbCompletionRe.FindSubmatch(m)
		if slices.Contains(partials, string(g[2])) {
			return []byte(`"completion":` + string(g[1]) + `"PARTIAL"`)
		}
		return m
	})
}

// setbReplFamily compares the replicating child's node instances: nodeInstancesFamily (niIngestRender) of the sides'
// windows, and the completion C computed for the open chart's partial answer on that side written PARTIAL
// (setbReplSide.partials): its second is each side's own.
func setbReplFamily(sides [2]setbReplSide) v2Family {
	fam := nodeInstancesFamily([2]niSide{sides[0].niSide, sides[1].niSide})
	ingest := fam.render
	fam.render = func(i int, flight [2]int64, body []byte) []byte {
		return setbPartialRender(sides[i].partials(), ingest(i, flight, body))
	}
	return fam
}

// setbReplStages are the replicating child's stages, in order, and what C printed of its ingestion in each (H41's
// SA-B probe p1; rrdhost-status.c:178-185, :212-217; api_v2_contexts.c:353-361):
//   - open: the done chart replicated to its end and collected, the open one defined, its request never answered:
//     replicating (a chart replicates), in progress, one instance, the completion the last answer's 100
//     (pluginsd_replication.c:478: the done chart's answer started streaming); one metric collected, two stored;
//   - partial: the open chart answered to base+30 without starting streaming: the completion its answer's
//     (setbCompletion, PARTIAL), its metric collected too (the answer's RSETs, pluginsd_replication.c:270-276);
//   - returned: the child left and came back sending nothing: replicating with nothing in progress (no metric
//     collected yet, rrdhost-status.c:179-182; the receiver's count zeroed at its end, stream-receiver.c:1355-1361),
//     the completion still the partial answer's (the host keeps it; only a host revived from ARCHIVED starts at 100,
//     database/rrdhost.c:448, :821), its second connection.
var setbReplStages = []string{"open", "partial", "returned"}

// setbReplIngest are the replicating child's ingestion members C printed at stage (setbReplStages): the connection
// count, the collected counts and the replication.
func setbReplIngest(stage string) (id, collected, replication string) {
	switch stage {
	case "open":
		return "1", "1", `{"in_progress":true,"completion":100,"instances":1}`
	case "partial":
		return "1", "2", `{"in_progress":true,"completion":"PARTIAL","instances":1}`
	case "returned":
		return "2", "0", `{"in_progress":false,"completion":"PARTIAL","instances":0}`
	}
	return "", "", ""
}

// setbReplFacts are the facts of `/api/v3/node_instances?scope_nodes=parity-repl` at stage, for the fixture's base:
// the child alone, numbered 0, with one instance (niHostInstance: it streams no function, so `functions` is {} until
// it is archived); its database in the stream's memory mode, the two charts' one metric each (open and partial stored
// from base, the first point's start), stale while it replicates (rrdhost-status.c:385-390), until now; its ingestion
// a child's that replicates (setbReplIngest), with its source (it is replicating, api_v2_contexts.c:363-376): its
// negotiated capabilities (stream.CapsReplication); the agent receiving it (rrd-metadata.c:35-44).
func setbReplFacts(stage string, base int64) func(Value) error {
	_, db, ingest := niChildPaths(0)
	id, collected, replication := setbReplIngest(stage)
	return dashGuard([]dashFact{niMembers, dashAbsent("nodes", "[1]")},
		niHostInstance(niShort, 0, 0, setbReplHost.MachineGUID, setbReplHost.Hostname, true),
		dashMembers(db, "status", `"online"`, "liveness", `"stale"`, "mode", `"ram"`, "first_time",
			strconv.FormatInt(base, 10), "last_time", `"NOW"`, "metrics", "2", "instances", "2", "contexts", "2"),
		[]dashFact{dashKeys("id hops type status since age metrics instances contexts replication source", ingest...)},
		dashMembers(ingest, "id", id, "hops", "1", "type", `"child"`, "status", `"replicating"`, "since", `"CONNECTED"`,
			"age", `"NOW-SINCE"`, "metrics", collected, "instances", collected, "contexts", collected,
			"replication", replication, "source", `{"local":"[127.0.0.1]:LISTEN","remote":"[127.0.0.1]:PEER",`+
				`"capabilities":["VCAPS","HLABELS","CLABELS","REPLICATION","INTERPOLATED"]}`),
		niAgentFactsOf(parentIdentity.MachineGUID, `{"total":2,"receiving":1,"sending":0,"archived":0}`))
}

// setbReplRow is the replicating child's node-instance request at stage.
func setbReplRow(stage string, base int64) v2Req {
	return v2Req{name: "v3-ni", target: "/api/v3/node_instances?scope_nodes=" + setbReplHost.Hostname, status: "200",
		guard: setbReplFacts(stage, base)}
}

// setbReplGuard is the guard of the oracle's table at stage, given the oracle's side (its partial answer's texts):
// localhost initializing, then the child, replicating on its receiver (its reason the handshake's), without a sender,
// of severity normal (function-netdata-streaming.c:107-138), its replication as setbReplIngest says (:210-211): the
// completion 100, or one of the side's partials.
func setbReplGuard(stage string, oracle setbReplSide) func(Value) error {
	id, collected, _ := setbReplIngest(stage)
	instances := "1"
	if stage == "returned" {
		instances = "0"
	}
	child := fnStreamingRow{"Node": strconv.Quote(setbReplHost.Hostname), "rowOptions": `{"severity":"normal"}`,
		"InStatus": `"replicating"`, "OutStatus": `"disabled"`, "InConnections": id, "InReason": `"CONNECTED"`,
		"InHops": "1", "InReplInstances": instances, "CollectedMetrics": collected, "dbMetrics": "2",
		"InCapabilities": `["VCAPS","HLABELS","CLABELS","REPLICATION","INTERPOLATED"]`}
	if stage == "open" {
		child["InReplCompletion"] = "100"
	}
	rows := fnStreamingRows(fnStreamingRow{"Node": strconv.Quote(parentIdentity.Hostname), "InReason": `"LOCALHOST"`,
		"InStatus": `"initializing"`, "OutStatus": `"disabled"`}, child)
	return func(v Value) error {
		if err := rows(v); err != nil {
			return err
		}
		if stage != "open" {
			return setbReplCompletionFact(v, oracle)
		}
		return nil
	}
}

// setbReplCompletionFact holds the child's InReplCompletion to one of the side's partials and the column's `max` to
// the 100 C passes (function-netdata-streaming.c:655-659).
func setbReplCompletionFact(v Value, side setbReplSide) error {
	r := setbRow(v, setbReplHost.Hostname)
	c, err := fnStreamingCell(v, r, "InReplCompletion")
	if err != nil {
		return err
	}
	if !slices.Contains(side.partials(), c.Text) || c.Kind != KindNumber {
		return fmt.Errorf("row %d: InReplCompletion is %s, none of the partial answer's %q", r, c, side.partials())
	}
	if m, err := dashAt(v, "columns", "InReplCompletion", "max"); err != nil || m.String() != "100" {
		return fmt.Errorf("columns.InReplCompletion.max is %s (%v), want 100", m, err)
	}
	return nil
}

// setbReplFnFacts are what side's table is held to besides fnStreamingFacts: the child's InSince the second of its
// connection (opened; rrdhost-status.c:169), and after the partial answer its completion (setbReplCompletionFact).
func setbReplFnFacts(stage string, side setbReplSide) func(Value) error {
	return func(v Value) error {
		r := setbRow(v, setbReplHost.Hostname)
		if r < 0 {
			return fmt.Errorf("no row of %s", setbReplHost.Hostname)
		}
		cells, err := setbCells(v, r, "InSince")
		if err != nil {
			return err
		}
		if !setbWithin(cells["InSince"], side.opened) {
			return fmt.Errorf("row %d: InSince %d is no second of the connection %v", r, cells["InSince"], side.opened)
		}
		if stage != "open" {
			return setbReplCompletionFact(v, side)
		}
		return nil
	}
}

// setbReplAsk is the table's comparison at stage: setbReplGuard, InReplCompletion masked as a volatile column once
// the partial answer set it (each side's own second), held per side by setbReplFnFacts.
func setbReplAsk(stage string, sides [2]setbReplSide) fnStreamingAsk {
	ask := fnStreamingAsk{target: "/api/v1/function?function=netdata-streaming", guard: setbReplGuard(stage, sides[0]),
		settle: dashSettle, facts: func(i int, v Value, _ [2]int64) error { return setbReplFnFacts(stage, sides[i])(v) }}
	if stage != "open" {
		ask.volatile = []string{"InReplCompletion"}
	}
	return ask
}

// setbReplRequest is a replication request a parent sent: the chart, whether to start streaming, its window.
type setbReplRequest struct {
	chart         string
	start         bool
	after, before int64
}

// setbReadRequest reads the parent's lines until its next replication request for chart (`REPLAY_CHART "<chart>"
// "<true|false>" <after> <before>`, stream-replication-receiver.c:113-122).
func setbReadRequest(conn *stream.Conn, chart string, limit time.Duration) (setbReplRequest, error) {
	deadline := time.Now().Add(limit)
	for {
		l, err := conn.ReadLine(deadline)
		if err != nil {
			return setbReplRequest{}, fmt.Errorf("no request for %s: %v", chart, err)
		}
		w := strings.Fields(strings.ReplaceAll(l, `"`, " "))
		if len(w) < 5 || w[0] != "REPLAY_CHART" || w[1] != chart {
			continue
		}
		after, err1 := strconv.ParseInt(w[3], 10, 64)
		before, err2 := strconv.ParseInt(w[4], 10, 64)
		if err1 != nil || err2 != nil {
			return setbReplRequest{}, fmt.Errorf("cannot read %q", l)
		}
		return setbReplRequest{chart, w[2] == "true", after, before}, nil
	}
}

// setbAnswer writes the child's answer to r up to `to` (the request's end when 0), from ch's rows, as a C child writes
// it (stream-replication-sender.c:613-700: RBEGIN, a row per point, REND with the window sent), streaming started only
// when r asks it and the answer reaches its end.
func setbAnswer(conn *stream.Conn, ch fixture.Chart, r setbReplRequest, to, first, last int64) error {
	if to == 0 {
		to = r.before
	}
	start := r.start && to == r.before
	conn.Linef("RBEGIN '%s'", ch.ID)
	for _, row := range ch.ReplayWindow(r.after, to) {
		conn.Linef("RBEGIN '%s' %d %d %d", ch.ID, row.T-1, row.T, last)
		for _, d := range row.Dims {
			conn.Linef("RSET '%s' %s %s", d.ID, d.Collected, d.Flags)
		}
	}
	conn.Linef("REND 1 %d %d %t %d %d %d", first, last, start, r.after, to, last)
	return conn.Flush()
}

// setbReplCharts are the replicating child's two charts: one dimension each, a value a second from base+1 to base+60.
func setbReplCharts(base int64) (done, open fixture.Chart) {
	series := func(id string, mod int) fixture.Chart {
		return fixture.Series(id, id, base, 60, 1, func(i int) string { return strconv.Itoa(i % mod) },
			func(int) string { return stream.FlagNotAnomalous })
	}
	return series(setbReplDone, 7), series(setbReplOpen, 5)
}

// setbReplLink connects the replicating child to each side (dashLinkAs with stream.CapsReplication: a child with
// replication sends its charts' retention and the parent asks for it).
func setbReplLink(t *testing.T, p *Pair) (conns [2]*stream.Conn, opened [2][2]int64) {
	t.Helper()
	for i, side := range p.Each() {
		from := time.Now().Unix()
		conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, setbReplHost, stream.CapsReplication)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		t.Cleanup(func() { _ = conn.Close() })
		conns[i], opened[i] = conn, [2]int64{from, time.Now().Unix()}
	}
	return conns, opened
}

// setbCollectedNone waits until each side's node instances read the host's collected metrics 0: the contexts worker
// took them down after its receiver left (rrdcontext-worker.c:1125-1127; 0.6 to 0.8 s after the close in H39's probe).
// The polls are tagged harness=wait. The oracle's failure ends the case; a candidate's is reported.
func setbCollectedNone(t *testing.T, p *Pair, host stream.HostInfo) {
	t.Helper()
	target := "/api/v3/node_instances?scope_nodes=" + host.Hostname + "&harness=wait"
	for _, side := range p.Each() {
		got := ""
		if pollUntil(niGoneWait, func() bool {
			b, err := v2Exchange(side.Daemon.Addr, v2Req{target: target})
			if err != nil {
				got = err.Error()
				return false
			}
			v, err := ParseJSON(httpBody(b))
			if err != nil {
				got = err.Error()
				return false
			}
			m, err := dashAt(v, "nodes", "[0]", "instances", "[0]", "ingest", "metrics")
			got = m.String()
			return err == nil && m.Text == "0"
		}) {
			continue
		}
		if side.Role == Oracle {
			t.Fatalf("oracle: %s still collects %s metrics after %v", host.Hostname, got, niGoneWait)
		}
		t.Errorf("candidate: %s still collects %s metrics after %v", host.Hostname, got, niGoneWait)
	}
}

// setbReplCompare runs stage's rows on p: the node instances and the table.
func setbReplCompare(t *testing.T, p *Pair, stage string, base int64, sides [2]setbReplSide) {
	t.Run(stage, func(t *testing.T) {
		r := setbReplRow(stage, base)
		t.Run(r.name, func(t *testing.T) { compareV2(t, p, r, setbReplFamily(sides)) })
		fnStreamingCompareWith(t, p, setbReplAsk(stage, sides))
	})
}

// setbRepl (TestNodeInstancesSetB/replicating; plan 4.2 `replicating`, 7.3 question 16): a parent pair (dashPair's,
// with fn.http's bearer tokens) and a fixture child with REPLICATION (setbReplHost), the same base on both sides:
// its done chart replicated to its end (each request answered whole), its open chart defined and its request read
// but not answered (stage open); then that request answered to base+30 without starting streaming, and the
// parent's request for the rest read (partial); then the child closes, each side reads it stale, stamps its
// detach (niGoneWindows) and takes its collected metrics down, and it connects again sending nothing (returned).
func setbRepl(t *testing.T) {
	p := startPairWith(t, dashPairOptions(daemon.Options{}), parentIdentity, binaries(t), [2]string{},
		[2]Role{Oracle, Candidate}, fnWriteTokens)
	niReady(p)
	base := dashBase()
	done, open := setbReplCharts(base)
	conns, opened := setbReplLink(t, p)
	var sides [2]setbReplSide
	var asked [2]setbReplRequest
	for i, side := range p.Each() {
		sides[i].niSide = niSides(p, opened)[i]
		c := conns[i]
		done.Define(c)
		c.ChartDefinitionEnd(base, base+60, base+60)
		if err := c.Flush(); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		// each request of the done chart answered whole, until one that starts streaming
		started := int64(0)
		for {
			r, err := setbReadRequest(c, setbReplDone, 30*time.Second)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			if started == 0 || r.after < started {
				started = r.after
			}
			if err := setbAnswer(c, done, r, 0, base, base+60); err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			if r.start {
				break
			}
		}
		open.Define(c)
		c.ChartDefinitionEnd(base, base+60, base+60)
		if err := c.Flush(); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		r, err := setbReadRequest(c, setbReplOpen, 30*time.Second)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		asked[i] = r
		sides[i].started = min(started, r.after)
	}
	time.Sleep(2 * time.Second)
	setbReplCompare(t, p, "open", base, sides)
	for i, side := range p.Each() {
		from := time.Now().Unix()
		if err := setbAnswer(conns[i], open, asked[i], base+30, base, base+60); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		if _, err := setbReadRequest(conns[i], setbReplOpen, 30*time.Second); err != nil {
			t.Fatalf("%s: after the partial answer: %v", side.Role, err)
		}
		sides[i].current, sides[i].answered = base+30, [2]int64{from, time.Now().Unix()}
	}
	time.Sleep(2 * time.Second)
	setbReplCompare(t, p, "partial", base, sides)
	// the return: the receiver's end, its detach stamped, the collected counts down, then a connection sending nothing
	after := time.Now().Unix()
	time.Sleep(time.Until(time.Unix(after+1, 0)))
	from := time.Now().Unix()
	dashGone(t, p, setbReplHost, conns)
	niGoneWindows(t, p, setbReplHost, from)
	setbCollectedNone(t, p, setbReplHost)
	_, again := setbReplLink(t, p)
	for i := range sides {
		sides[i].opened = again[i]
	}
	time.Sleep(2 * time.Second)
	setbReplCompare(t, p, "returned", base, sides)
}

// ---------------------------------------------------------------------------------------------------------------
// relay: a proxied child whose charts replicate up the stream (setbRelay)

// The proxied child's three charts: X is asked a part of its retention and left replicating up the stream, Y a window
// after the wall clock (no query, streaming starts), Z starts at once.
const (
	setbRelayX = "relay.x"
	setbRelayY = "relay.y"
	setbRelayZ = "relay.z"
)

var setbRelayCharts = []string{setbRelayX, setbRelayY, setbRelayZ}

// setbRelaySide is what the relay rows take from one side's own run: niStreamSide's windows (the child's connection,
// opened; the proxy's sender's, connected; the scripted parent's port, stub) and the sender's replication stamps once
// X's partial request was answered: the request's `after` (oldest: oldest_request_after_t,
// stream-replication-sender.c:1312-1313) and its answer's end (latest: latest_completed_before_t, the query's
// `before`, :301, as the answer's REND says it, :684-694).
type setbRelaySide struct {
	niStreamSide
	oldest, latest int64
}

// completion is the sender's completion C writes at the second now (setbCompletion of the stamps), empty before X's
// answer.
func (s setbRelaySide) completion(now int64) string {
	if s.oldest == 0 {
		return ""
	}
	return setbCompletion(s.oldest, s.latest, now)
}

// setbRelayWords are setbRelayFamily's own replacements in a `stream` object of side's answer whose walk's clock is
// now (clocked: the body has one), each by its path in the object: the replication's completion COMPLETION where it
// is the stamps' at that clock (setbRelaySide.completion; rrdhost-status.c:81-97, :269: the walk's `now`,
// api_v2_contexts.c:495), and each stream path entry's `since` OPENED where it is a second of the child's connection
// (opened: the proxied host's own entry is stamped at its receiver's attach, streaming/stream-path.c:128-136).
func setbRelayWords(side setbRelaySide, obj []byte, now int64, clocked bool) []byte {
	return jsonRewrite(obj, nil, func(path []string, value []byte) ([]byte, bool) {
		at := strings.Join(path, ".")
		switch {
		case at == "replication.completion":
			if want := side.completion(now); clocked && want != "" && string(value) == want {
				return []byte(`"COMPLETION"`), true
			}
		case len(path) == 4 && path[0] == "destination" && path[1] == "streaming_path" && path[3] == "since":
			if n, err := strconv.ParseInt(string(value), 10, 64); err == nil && side.opened[1] > 0 &&
				n >= side.opened[0] && n <= side.opened[1] {
				return []byte(`"OPENED"`), true
			}
		}
		return nil, false
	})
}

// setbRelayFamily compares the relay's node instances: nodeInstancesFamily outside the `stream` objects (the child's
// connection by its opened window), and each `stream` by niStreamRender after setbRelayWords (niOutsideStream).
func setbRelayFamily(sides [2]setbRelaySide) v2Family {
	var ni [2]niSide
	var ns [2]niStreamSide
	for i := range sides {
		ni[i], ns[i] = sides[i].niSide, sides[i].niStreamSide
	}
	fam := nodeInstancesFamily(ni)
	inside := niStreamRender(ns)
	fam.render = niOutsideStream(fam.render, func(i int, flight [2]int64, body []byte, path []string, obj []byte) []byte {
		n, clocked := niNow(body)
		return inside(i, flight, body, path, setbRelayWords(sides[i], obj, n, clocked))
	})
	return fam
}

// setbRelayNegotiated are the capabilities the proxy's sender holds once the scripted parent answered with what it
// offered less compression and IEEE754 (setbRelayScript): niStreamOurs less V1, V2, VN and IEEE754
// (stream-capabilities.c:149-175).
const setbRelayNegotiated = `["VCAPS","HLABELS","CLAIM","CLABELS","FUNCTIONS","FUNCDEL","REPLICATION","BINARY",` +
	`"INTERPOLATED","DYNCFG","SLOTS","PROGRESS","NODEID","PATHS","FLOATBASELINE"]`

// setbRelayOurs are the capabilities the proxy lists in its own stream path entry of the proxied host: its own, with
// compression on (stream_our_capabilities(), stream-capabilities.c:101-147), in C's order of names.
const setbRelayOurs = `["V1","V2","VN","VCAPS","HLABELS","CLAIM","CLABELS","LZ4","FUNCTIONS","FUNCDEL","REPLICATION",` +
	`"BINARY","INTERPOLATED","IEEE754","DYNCFG","SLOTS","ZSTD","GZIP","BROTLI","PROGRESS","NODEID","PATHS",` +
	`"FLOATBASELINE"]`

// setbRelayStages are the relay's stages, in order, and what C printed of the proxied host's `stream` in each (SA-B's
// probe p1; rrdhost-status.c:262-275, :81-97): the stream's status, and its replication's in progress, completion and
// instances:
//   - stuck: X and Y defined upstream, no request sent for them (the sender counts a chart replicating from its
//     definition, streaming/protocol/command-chart-definition.c:131); Z started: replicating, two instances, the
//     completion 100 (no request with an `after`, rrdhost-status.c:84-85);
//   - partial: X asked (base-60, base-30] without streaming, answered: the stamps set, the completion theirs;
//   - noquery: Y asked a window after the wall clock: no query (stream-replication-sender.c:586-591, :144-148: no
//     stamp written, :292-301), streaming starts, one instance left, the completion unchanged in form;
//   - online: X asked to start streaming: online, nothing replicating, 100.
var setbRelayStages = []string{"stuck", "partial", "noquery", "online"}

// setbRelayState is the proxied host's stream status and replication C printed at stage (setbRelayStages).
func setbRelayState(stage string) (status, replication string) {
	switch stage {
	case "stuck":
		return "replicating", `{"in_progress":true,"completion":100,"instances":2}`
	case "partial":
		return "replicating", `{"in_progress":true,"completion":"COMPLETION","instances":2}`
	case "noquery":
		return "replicating", `{"in_progress":true,"completion":"COMPLETION","instances":1}`
	case "online":
		return "online", `{"in_progress":false,"completion":100,"instances":0}`
	}
	return "", ""
}

// setbRelayFacts are the facts of the relay's `/api/v3/node_instances` at stage, the scripted parent listening on port,
// for the child's base: two hosts in creation order.
//   - localhost, initializing with the pulse off (niLocalFacts) and with no `stream`: `[stream] enabled = no` gives it
//     no sender (api_v2_contexts.c:383-384; only the child's section proxies);
//   - the proxied child: its node and instance (niHostInstance's members but `stream` between `ingest` and `ml`); its
//     database in the proxy's own dbengine, its three charts' metric each, from base-59 (the replicated windows start
//     at the retention's first point, base-60's next), until now; its ingestion a child's online on its receiver
//     (the replicated charts collected), with its source; its `stream`: the sender's first connection, two hops (the
//     child's one and the proxy's, stream-connector.c:276), its status and replication at stage (setbRelayState), the
//     socket's ends (SOCKET, the parent's port), the negotiated names, no compression, no data block sent (the
//     child's blocks are an hour old), the definitions' bytes above 0 and the replication answers' (SENT), no
//     function's; the parent connected once (niStreamParent: SOCKET CONNECTED, ranked alone, its probe unanswered);
//     the stream path the proxy's own entry for the host: hops 1, since the child's connection (OPENED), its first
//     time the database's (DB-FIRST), the proxy's own capabilities (setbRelayOurs);
//   - the agent's nodes: two, one received, one sending (rrd-metadata.c:25-45).
func setbRelayFacts(stage, port string, base int64) func(Value) error {
	node := []string{"nodes", "[1]"}
	inst := append(slices.Clone(node), "instances", "[0]")
	db, ingest, st := append(slices.Clone(inst), "db"), append(slices.Clone(inst), "ingest"),
		append(slices.Clone(inst), "stream")
	dst := append(slices.Clone(st), "destination")
	status, replication := setbRelayState(stage)
	r := &niStreamRun{}
	return dashGuard([]dashFact{niMembers, dashAbsent("nodes", "[2]"),
		dashKeys("st db ingest ml health functions capabilities dyncfg", "nodes", "[0]", "instances", "[0]")},
		niLocalFacts(niShort),
		dashNode(1, 1, proxiedHost.MachineGUID, proxiedHost.Hostname),
		[]dashFact{dashKeys("mg nm ni instances", node...), dashAbsent(append(slices.Clone(node), "instances", "[1]")...),
			dashKeys("st db ingest stream ml health functions capabilities dyncfg", inst...)},
		dashMembers(inst, "st", `{"ai":0,"code":200,"msg":""}`, "ml", `{"status":"disabled","type":"disabled"}`,
			"health", `{"status":"disabled"}`, "functions", "{}", "capabilities", nodeCaps(false),
			"dyncfg", `{"status":"unavailable"}`),
		dashMembers(db, "status", `"online"`, "liveness", `"live"`, "mode", `"dbengine"`, "first_time",
			strconv.FormatInt(base-59, 10), "last_time", `"NOW"`, "metrics", "3", "instances", "3", "contexts", "3"),
		[]dashFact{dashKeys("id hops type status since age metrics instances contexts source", ingest...)},
		dashMembers(ingest, "id", "1", "hops", "1", "type", `"child"`, "status", `"online"`, "since", `"CONNECTED"`,
			"age", `"NOW-SINCE"`, "metrics", "3", "instances", "3", "contexts", "3",
			"source", `{"local":"[127.0.0.1]:LISTEN","remote":"[127.0.0.1]:PEER",`+
				`"capabilities":["VCAPS","HLABELS","CLABELS","REPLICATION","INTERPOLATED"]}`),
		[]dashFact{dashKeys("id hops status since age replication destination", st...),
			dashKeys("local remote capabilities traffic parents streaming_path", dst...),
			dashKeys("compression data metadata functions replication", append(slices.Clone(dst), "traffic")...),
			dashAbove(0, append(slices.Clone(dst), "traffic", "metadata")...)},
		dashMembers(st, "id", "1", "hops", "2", "status", strconv.Quote(status), "since", `"CONNECTED"`,
			"age", `"NOW-SINCE"`, "replication", replication),
		dashMembers(dst, "local", `"[127.0.0.1]:SOCKET"`, "remote", `"[127.0.0.1]:`+port+`"`,
			"capabilities", setbRelayNegotiated,
			"parents", "["+niStreamParent(r, port, 2, "CONNECTED", `"last_handshake":"SOCKET CONNECTED","batch":1,`+
				`"order":1,"random":false,"info":false,"skipped":false`)+"]",
			"streaming_path", `[{"version":1,"hostname":"`+parentIdentity.Hostname+`","host_id":"`+
				parentIdentity.MachineGUID+`","node_id":null,"claim_id":null,"hops":1,"since":"OPENED",`+
				`"first_time_t":"DB-FIRST","start_time":0,"shutdown_time":0,"capabilities":`+setbRelayOurs+
				`,"flags":[]}]`),
		dashMembers(append(slices.Clone(dst), "traffic"), "compression", "false", "data", "0", "functions", "0",
			"replication", `"SENT"`),
		niAgentFactsOf(parentIdentity.MachineGUID, `{"total":2,"receiving":1,"sending":1,"archived":0}`))
}

// setbRelayRow is the relay's node-instance request at stage: every host.
func setbRelayRow(stage, port string, base int64) v2Req {
	return v2Req{name: "v3-ni", target: "/api/v3/node_instances", status: "200", guard: setbRelayFacts(stage, port, base)}
}

// setbRelayVolatile are the relay table's columns masked besides fnStreamingVolatile, held per side by setbRelayFnFacts:
// the sender's own port and the parents' last attempt (fnStreamingOutVolatile's), and the sender's completion once X's
// answer set the stamps (each side's clock).
func setbRelayVolatile(stage string) []string {
	out := []string{"OutLocalPort", "OutAttemptSince", "OutAttemptAge"}
	if stage == "partial" || stage == "noquery" {
		out = append(out, "OutReplCompletion")
	}
	return out
}

// setbRelayGuard is the guard of the oracle's table at stage, the scripted parent listening on port: localhost
// (initializing, no sender), then the proxied child, online on its receiver, its sender connected once with two hops
// to the parent's port, plain and uncompressed, its status and instances as setbRelayState says, no data block sent,
// the parent's attempt SOCKET CONNECTED, of severity normal (function-netdata-streaming.c:107-138, :229-287); its
// completion 100, or the oracle's stamps' at the table's clock (setbRelayCompletionFact).
func setbRelayGuard(stage, port string, oracle setbRelaySide) func(Value) error {
	status, _ := setbRelayState(stage)
	instances := map[string]string{"stuck": "2", "partial": "2", "noquery": "1", "online": "0"}[stage]
	child := fnStreamingRow{"Node": strconv.Quote(proxiedHost.Hostname), "rowOptions": `{"severity":"normal"}`,
		"InStatus": `"online"`, "OutStatus": strconv.Quote(status), "InConnections": "1", "InReason": `"CONNECTED"`,
		"InHops": "1", "OutConnections": "1", "OutReason": `"CONNECTED"`, "OutHops": "2", "OutReplInstances": instances,
		"OutRemotePort": port, "OutSSL": `"PLAIN"`, "OutCompression": `"UNCOMPRESSED"`, "OutTrafficData": "0",
		"OutAttemptHandshake": `["SOCKET CONNECTED"]`, "OutCapabilities": setbRelayNegotiated, "dbMetrics": "3",
		"CollectedMetrics": "3"}
	masked := slices.Contains(setbRelayVolatile(stage), "OutReplCompletion")
	if !masked {
		child["OutReplCompletion"] = "100"
	}
	rows := fnStreamingRows(fnStreamingRow{"Node": strconv.Quote(parentIdentity.Hostname), "InReason": `"LOCALHOST"`,
		"InStatus": `"initializing"`, "OutStatus": `"disabled"`}, child)
	return func(v Value) error {
		if err := rows(v); err != nil {
			return err
		}
		if masked {
			return setbRelayCompletionFact(v, oracle)
		}
		return nil
	}
}

// setbRelayCompletionFact holds the proxied child's OutReplCompletion to the side's stamps at the table's clock (its
// InSince and InAge: the handler's one clock, function-netdata-streaming.c:22, :185-192, :253) and the column's `max`
// to the 100 C passes (:754-758).
func setbRelayCompletionFact(v Value, side setbRelaySide) error {
	r := setbRow(v, proxiedHost.Hostname)
	cells, err := setbCells(v, r, "InSince", "InAge")
	if err != nil {
		return err
	}
	clock := cells["InSince"]/1000 + cells["InAge"]
	c, err := fnStreamingCell(v, r, "OutReplCompletion")
	if err != nil {
		return err
	}
	if want := side.completion(clock); want == "" || c.Text != want || c.Kind != KindNumber {
		return fmt.Errorf("row %d: OutReplCompletion is %s, the stamps' at the clock %d are %q", r, c, clock, want)
	}
	if m, err := dashAt(v, "columns", "OutReplCompletion", "max"); err != nil || m.String() != "100" {
		return fmt.Errorf("columns.OutReplCompletion.max is %s (%v), want 100", m, err)
	}
	return nil
}

// setbRelayFnFacts are what side's table is held to besides fnStreamingFacts, on the proxied child's row: its InSince
// the second of its connection (opened); its OutSince the sender's connection's (connected; stream-sender.c:365); the
// parents' last attempt the milliseconds of a moment of that connection's pass and its age the seconds from its second
// to the table's clock (:271-287), each `max` the largest cell; OutLocalPort the sender socket's own port, neither the
// agent's nor the parent's, without a `max` (:257, :775); and once X's answer set the stamps, the completion
// (setbRelayCompletionFact).
func setbRelayFnFacts(stage string, side setbRelaySide) func(Value) error {
	return func(v Value) error {
		r := setbRow(v, proxiedHost.Hostname)
		if r < 0 {
			return fmt.Errorf("no row of %s", proxiedHost.Hostname)
		}
		cells, err := setbCells(v, r, "InSince", "InAge", "OutSince", "OutAttemptSince", "OutAttemptAge",
			"OutLocalPort")
		if err != nil {
			return err
		}
		clock := cells["InSince"]/1000 + cells["InAge"]
		switch attempt := cells["OutAttemptSince"]; {
		case !setbWithin(cells["InSince"], side.opened):
			return fmt.Errorf("row %d: InSince %d is no second of the child's connection %v", r, cells["InSince"],
				side.opened)
		case !setbWithin(cells["OutSince"], side.connected):
			return fmt.Errorf("row %d: OutSince %d is no second of the sender's connection %v", r, cells["OutSince"],
				side.connected)
		case attempt/1000 < side.connected[0] || attempt/1000 > side.connected[1] ||
			attempt/1000+cells["OutAttemptAge"] != clock:
			return fmt.Errorf("row %d: OutAttemptSince %d and OutAttemptAge %d: not a moment of the connection %v and "+
				"the seconds from it to the clock %d", r, attempt, cells["OutAttemptAge"], side.connected, clock)
		}
		if p := strconv.FormatInt(cells["OutLocalPort"], 10); cells["OutLocalPort"] <= 0 ||
			cells["OutLocalPort"] > 65535 || p == side.listen || p == side.stub {
			return fmt.Errorf("row %d: OutLocalPort %s is not the sender socket's own port", r, p)
		}
		if _, err := dashAt(v, "columns", "OutLocalPort", "max"); err == nil {
			return fmt.Errorf("columns.OutLocalPort has a max: C passes none (function-netdata-streaming.c:775)")
		}
		for _, name := range []string{"OutAttemptSince", "OutAttemptAge"} {
			if m, err := dashAt(v, "columns", name, "max"); err != nil || m.Text != strconv.FormatInt(cells[name], 10) {
				return fmt.Errorf("columns.%s.max is %s, the one cell is %d", name, m, cells[name])
			}
		}
		if slices.Contains(setbRelayVolatile(stage), "OutReplCompletion") {
			return setbRelayCompletionFact(v, side)
		}
		return nil
	}
}

// setbRelayAsk is the relay table's comparison at stage, the scripted parent listening on port.
func setbRelayAsk(stage, port string, sides [2]setbRelaySide) fnStreamingAsk {
	return fnStreamingAsk{target: "/api/v1/function?function=netdata-streaming",
		guard: setbRelayGuard(stage, port, sides[0]), settle: dashSettle, volatile: setbRelayVolatile(stage),
		facts: func(i int, v Value, _ [2]int64) error { return setbRelayFnFacts(stage, sides[i])(v) }}
}

// setbRelayScript is the scripted parent's answer: what the proxy offered less compression and IEEE754 (plaintext and
// decimal numbers, so the proxy's answers are readable), X and Y left without a request at their definitions (the
// case sends theirs), any other chart started at once.
func setbRelayScript(r stream.Request) stream.Answer {
	return stream.Answer{Reply: stream.VCaps(r.Caps() &^ (stream.CapsCompression | stream.CapIEEE754)),
		Replay: func(ev stream.ReplayEvent) []string {
			if ev.Answer || ev.Chart == setbRelayX || ev.Chart == setbRelayY {
				return nil
			}
			return []string{`REPLAY_CHART "` + ev.Chart + `" "true" 0 0`}
		}}
}

// setbRelayRendRe is a replication answer's last line as the proxy writes it in decimal (stream-replication-sender.c:
// 674-694): the update every, the retention, whether streaming starts, the window sent and the wall clock.
var setbRelayRendRe = regexp.MustCompile(`(?m)^REND \d+ \d+ \d+ (true |false) (\d+) (\d+) \d+$`)

// setbRelayRends are the replication answers' windows of a session's data, in order: whether each starts streaming,
// its after and before.
func setbRelayRends(data []byte) [][3]string {
	var out [][3]string
	for _, g := range setbRelayRendRe.FindAllSubmatch(data, -1) {
		out = append(out, [3]string{strings.TrimSpace(string(g[1])), string(g[2]), string(g[3])})
	}
	return out
}

// setbRelaySend sends line to each session and waits until each has one more replication answer, which it hands back.
func setbRelaySend(t *testing.T, sessions [2]*stream.Session, line string) [2][3]string {
	t.Helper()
	var got [2][3]string
	for i, s := range sessions {
		before := len(setbRelayRends(s.Data()))
		if err := s.Send(line); err != nil {
			t.Fatalf("session %d: %v", i, err)
		}
		if !s.WaitData(func(b []byte) bool { return len(setbRelayRends(b)) > before }, 30*time.Second) {
			t.Fatalf("session %d: no answer to %s", i, line)
		}
		got[i] = setbRelayRends(s.Data())[before]
	}
	return got
}

// setbRelayCounted waits until each side's proxied host has a connected sender counted (stream `id` 1, not
// offline), and hands back the second each side was first seen so; the polls are tagged harness=wait.
func setbRelayCounted(t *testing.T, p *Pair) [2]int64 {
	t.Helper()
	var at [2]int64
	target := "/api/v3/node_instances?scope_nodes=" + proxiedHost.Hostname + "&harness=wait"
	for i, side := range p.Each() {
		got := ""
		ok := pollUntil(60*time.Second, func() bool {
			b, err := v2Exchange(side.Daemon.Addr, v2Req{target: target})
			if err != nil {
				got = err.Error()
				return false
			}
			v, err := ParseJSON(httpBody(b))
			if err != nil {
				got = err.Error()
				return false
			}
			s, err := dashAt(v, "nodes", "[0]", "instances", "[0]", "stream")
			got = s.String()
			id, err1 := dashMember(s, "id")
			st, err2 := dashMember(s, "status")
			return err == nil && err1 == nil && err2 == nil && id.Text != "0" && st.Text != "offline"
		})
		at[i] = time.Now().Unix()
		switch {
		case ok:
		case side.Role == Oracle:
			t.Fatalf("oracle: the proxied host's stream is not connected after 60 s: %s", got)
		default:
			t.Errorf("candidate: the proxied host's stream is not connected after 60 s: %s", got)
		}
	}
	return at
}

// setbRelayCompare runs stage's rows on p.
func setbRelayCompare(t *testing.T, p *Pair, stage, port string, base int64, sides [2]setbRelaySide) {
	t.Run(stage, func(t *testing.T) {
		r := setbRelayRow(stage, port, base)
		t.Run(r.name, func(t *testing.T) { compareV2(t, p, r, setbRelayFamily(sides)) })
		fnStreamingCompareWith(t, p, setbRelayAsk(stage, port, sides))
	})
}

// setbRelay (TestNodeInstancesSetB/relay; plan 4.2 `proxy`, and the sender's half of `replicating`): two proxies (C,
// then the candidate) with the bearer tokens, the pulse off and `[stream] enabled = no`, each proxying the fixture's
// proxied child (its `[<guid>]` section, as stream.proxy-transcript's) to one scripted parent (setbRelayScript); the
// child, with REPLICATION, defines three charts with an hour-old retention (base-60, base], serves the proxy's
// replication, and collects once at base+1 (the first collection starts the proxied host's sender) and at base+2
// (its definitions go up). Stages (setbRelayStages) once the parent's postponement after the handshake is over: stuck,
// then the case's own requests to both sessions: X (base-60, base-30] without streaming (partial), Y a window an hour
// after the wall clock (noquery), X to start streaming, and one more collection (online).
func setbRelay(t *testing.T) {
	stub, err := stream.StartParent(setbRelayScript)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { stub.Close() })
	_, port, _ := strings.Cut(stub.Addr(), ":")
	opts := daemon.Options{StorageTiers: 1, PulseOff: true, StreamSection: "    reconnect delay = 5\n",
		StreamExtra: fmt.Sprintf("\n[%s]\n    proxy enabled = yes\n    proxy destination = %s\n    proxy api key = %s\n",
			proxiedHost.MachineGUID, stub.Addr(), proxyUpKey)}
	p := startPairWith(t, opts, parentIdentity, binaries(t), [2]string{}, [2]Role{Oracle, Candidate}, fnWriteTokens)
	// each side's start and readiness: a time a sender takes while its agent boots reads START (niStreamSides)
	ready := niStreamSides(p, stub, time.Now().Unix())
	niReady(p)
	base := time.Now().Unix()/60*60 - 3600
	retention := map[string]stream.ReplayChart{}
	for _, id := range setbRelayCharts {
		retention[id] = stream.ReplayChart{FirstT: base - 60, LastT: base, UpdateEvery: 1}
	}
	rows := func(_ string, after, before int64) []stream.ReplayRow {
		var out []stream.ReplayRow
		for ts := after + 1; ts <= before; ts++ {
			out = append(out, stream.ReplayRow{T: ts, Dims: []stream.ReplayValue{
				{ID: "d", Collected: strconv.FormatInt(ts%97, 10), Flags: stream.FlagNotAnomalous}}})
		}
		return out
	}
	var children [2]*stream.Conn
	var opened [2][2]int64
	for i, side := range p.Each() {
		from := time.Now().Unix()
		c, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, proxiedHost, stream.CapsReplication)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		t.Cleanup(func() { _ = c.Close() })
		c.Serve(retention, base, rows)
		children[i], opened[i] = c, [2]int64{from, time.Now().Unix()}
	}
	all := func(write func(c *stream.Conn)) {
		for _, c := range children {
			if err := c.Burst(func() { write(c) }); err != nil {
				t.Fatal(err)
			}
		}
	}
	poke := func(at int64) {
		all(func(c *stream.Conn) {
			for _, id := range setbRelayCharts {
				c.Begin2Raw("", id, "1", strconv.FormatInt(at, 10), "#")
				c.Set2Raw("", "d", "1", "1", stream.FlagNotAnomalous)
				c.End2()
			}
		})
	}
	all(func(c *stream.Conn) {
		for _, id := range setbRelayCharts {
			c.DefineChart(stream.Chart{ID: id, Title: id, Units: "u", Family: "f", Context: id})
			c.Dimension("d", "absolute", 1, 1)
			c.ChartDefinitionEnd(base-60, base, base)
		}
	})
	for i, c := range children {
		if err := c.WaitGranted(setbRelayCharts, 30*time.Second); err != nil {
			t.Fatalf("side %d: %v", i, err)
		}
	}
	poke(base + 1)
	counted := setbRelayCounted(t, p)
	// the pass that connects a parent reads its clock before it probes it (stream-parents.c:608, :854)
	from := int64(0)
	if probes := stub.ProbeTimes(); len(probes) > 0 {
		from = probes[0].Unix() - 1
	}
	var sessions [2]*stream.Session
	if ss := stub.Sessions(); len(ss) == 2 {
		sessions = [2]*stream.Session{ss[0], ss[1]}
	} else {
		t.Fatalf("the scripted parent has %d sessions, want the two proxies'", len(ss))
	}
	for i, s := range sessions {
		if !s.WaitData(func(b []byte) bool { return strings.Contains(string(b), "\nOVERWRITE labels\n") },
			30*time.Second) {
			t.Fatalf("session %d: no host labels", i)
		}
	}
	poke(base + 2)
	for i, s := range sessions {
		if !s.WaitData(func(b []byte) bool { return strings.Count(string(b), "CHART_DEFINITION_END") >= 3 },
			30*time.Second) {
			t.Fatalf("session %d: no definitions", i)
		}
	}
	var sides [2]setbRelaySide
	for i, s := range ready {
		s.niSide.opened = opened[i]
		s.connected = [2]int64{from, counted[i]}
		sides[i].niStreamSide = s
	}
	// the parent stays postponed for 5 s after a handshake (stream-connector.c:236-238, stream-parents.c:110-117)
	last := sessions[0].At
	if sessions[1].At.After(last) {
		last = sessions[1].At
	}
	time.Sleep(time.Until(last.Add(6 * time.Second)))
	setbRelayCompare(t, p, "stuck", port, base, sides)
	oldest := base - 60
	got := setbRelaySend(t, sessions, fmt.Sprintf(`REPLAY_CHART "%s" "false" %d %d`, setbRelayX, oldest, base-30))
	// the sessions are the two proxies', in the order they connected: each answer's end names its own side's stamp
	for i := range sides {
		latest, err := strconv.ParseInt(got[i][2], 10, 64)
		if err != nil || got[i][0] != "false" {
			t.Fatalf("session %d: X's answer %q", i, got[i])
		}
		sides[i].oldest, sides[i].latest = oldest, latest
	}
	if got[0][2] != got[1][2] {
		t.Fatalf("the proxies' answers to X end at %s and %s: the sessions' sides cannot be told", got[0][2], got[1][2])
	}
	time.Sleep(time.Second)
	setbRelayCompare(t, p, "partial", port, base, sides)
	future := time.Now().Unix() + 3600
	setbRelaySend(t, sessions, fmt.Sprintf(`REPLAY_CHART "%s" "false" %d %d`, setbRelayY, future, future+10))
	time.Sleep(time.Second)
	setbRelayCompare(t, p, "noquery", port, base, sides)
	setbRelaySend(t, sessions, fmt.Sprintf(`REPLAY_CHART "%s" "true" 0 0`, setbRelayX))
	poke(base + 3)
	time.Sleep(2 * time.Second)
	setbRelayCompare(t, p, "online", port, base, sides)
}

// ---------------------------------------------------------------------------------------------------------------
// vnode: a virtual node beside a localhost that collects (setbVnode)

// setbVnodeFirsts are each side's seconds of the vnode's and localhost's first stored points (0 the oracle's), from
// the fake plugin's records of setbVnode's scenario: its three blocks of the vnode's chart, then two of localhost's;
// C stores no point at a chart's first collection (rrdset-collection.c:656-673), so each first stored point is its
// chart's second block's.
func setbVnodeFirsts(t *testing.T, ls [2]plugin.Layout) [2][2]int64 {
	t.Helper()
	var out [2][2]int64
	for i := range ls {
		starts, err := ls[i].Starts()
		if err != nil {
			t.Fatal(err)
		}
		if len(starts) == 0 {
			t.Fatalf("side %d: no plugin start", i)
		}
		secs := collectedSecs(starts[0])
		if len(secs) < 5 {
			t.Fatalf("side %d: the plugin collected %v, want five blocks", i, secs)
		}
		out[i] = [2]int64{secs[1], secs[4]}
	}
	return out
}

// setbFirstRe is a database's first time written as a number.
var setbFirstRe = regexp.MustCompile(`"first_time":(\s*)([0-9]+)`)

// setbVnodeFamily compares setbVnode's node instances: nodeInstancesFamily of the sides' start windows (nothing
// connects), and each database's first time read by the side's firsts: VNODE-FIRST the vnode's chart's second block,
// LOCAL-FIRST localhost's (setbVnodeFirsts); any other left as written.
func setbVnodeFamily(sides [2]niSide, firsts [2][2]int64) v2Family {
	fam := nodeInstancesFamily(sides)
	outside := fam.render
	fam.render = func(i int, flight [2]int64, body []byte) []byte {
		body = setbFirstRe.ReplaceAllFunc(outside(i, flight, body), func(m []byte) []byte {
			g := setbFirstRe.FindSubmatch(m)
			switch string(g[2]) {
			case strconv.FormatInt(firsts[i][0], 10):
				return []byte(`"first_time":` + string(g[1]) + `"VNODE-FIRST"`)
			case strconv.FormatInt(firsts[i][1], 10):
				return []byte(`"first_time":` + string(g[1]) + `"LOCAL-FIRST"`)
			}
			return m
		})
		return body
	}
	return fam
}

// setbVnodeFacts are the facts of setbVnode's `/api/v3/node_instances` (C in SA-B's probe p1): two hosts in creation
// order, neither with a `stream` (no destination):
//   - localhost, collecting: its database queryable and live in the agent's dbengine from its first stored point
//     (LOCAL-FIRST) until now, one metric of one chart; its ingestion localhost's, online since the agent started
//     (rrdhost-status.c:175-177: START); the built-in functions it registers (functions.c:5-30), its capabilities and
//     dyncfg online (aclk_capas.c:41-42; rrdhost-status.c:404-405);
//   - the vnode (vnodeGUID): its database the same in form from its own first point (VNODE-FIRST); its ingestion a
//     virtual node's (rrdhost-status.c:229-230), connection 0, one hop (rrdhost_ingestion_hops, :105), online since the
//     agent started (a local host, :175-177), its collected counts 1; no function, no dyncfg, the capabilities of a host
//     without functions (nodeCaps);
//   - the agent's nodes: two, the vnode counted received (a host other than localhost that is online,
//     rrd-metadata.c:35-44).
func setbVnodeFacts() func(Value) error {
	local := []string{"nodes", "[0]", "instances", "[0]"}
	vnode := []string{"nodes", "[1]", "instances", "[0]"}
	at := func(inst []string, key string) []string { return append(slices.Clone(inst), key) }
	keys := "id hops type status since age metrics instances contexts"
	db := func(first string) []string {
		return []string{"status", `"online"`, "liveness", `"live"`, "mode", `"dbengine"`, "first_time", first,
			"last_time", `"NOW"`, "metrics", "1", "instances", "1", "contexts", "1"}
	}
	return dashGuard([]dashFact{niMembers, dashAbsent("nodes", "[2]"),
		dashKeys("st db ingest ml health functions capabilities dyncfg", local...),
		dashKeys("netdata-streaming topology:streaming netdata-api-calls netdata-metrics-cardinality",
			at(local, "functions")...),
		dashKeys(keys, at(local, "ingest")...), dashKeys(keys, at(vnode, "ingest")...)},
		dashParent(0, 0),
		dashMembers(at(local, "db"), db(`"LOCAL-FIRST"`)...),
		dashMembers(at(local, "ingest"), "id", "0", "hops", "0", "type", `"localhost"`, "status", `"online"`,
			"since", `"START"`, "age", `"NOW-SINCE"`, "metrics", "1", "instances", "1", "contexts", "1"),
		dashMembers(local, "capabilities", nodeCaps(true), "dyncfg", `{"status":"online"}`),
		niHostInstance(niShort, 1, 1, vnodeGUID, vnodeName, true),
		dashMembers(at(vnode, "db"), db(`"VNODE-FIRST"`)...),
		dashMembers(at(vnode, "ingest"), "id", "0", "hops", "1", "type", `"virtual"`, "status", `"online"`,
			"since", `"START"`, "age", `"NOW-SINCE"`, "metrics", "1", "instances", "1", "contexts", "1"),
		niAgentFactsOf(parentIdentity.MachineGUID, `{"total":2,"receiving":1,"sending":0,"archived":0}`))
}

// setbVnodeGuard is the guard of the oracle's table: localhost and the vnode, both online without a sender, of
// severity normal; the vnode's inbound reason VIRTUAL NODE and its local address `localhost` (a virtual node,
// function-netdata-streaming.c:199-204, :212), one hop in, two out (the ingestion's + 1, rrdhost-status.c:246), no
// connection either way, its outbound reason NEVER CONNECTED (the reason's 0).
var setbVnodeGuard = fnStreamingRows(
	fnStreamingRow{"Node": strconv.Quote(parentIdentity.Hostname), "rowOptions": `{"severity":"normal"}`,
		"InReason": `"LOCALHOST"`, "InStatus": `"online"`, "OutStatus": `"disabled"`, "InHops": "0", "OutHops": "1",
		"InLocalIP": `"localhost"`, "dbMetrics": "1", "CollectedMetrics": "1"},
	fnStreamingRow{"Node": strconv.Quote(vnodeName), "rowOptions": `{"severity":"normal"}`, "InReason": `"VIRTUAL NODE"`,
		"InStatus": `"online"`, "OutStatus": `"disabled"`, "InHops": "1", "OutHops": "2", "InLocalIP": `"localhost"`,
		"InConnections": "0", "OutConnections": "0", "OutReason": `"NEVER CONNECTED"`, "dbMetrics": "1",
		"CollectedMetrics": "1"},
)

// setbVnodeFnFacts are what side's table is held to besides fnStreamingFacts and fnStreamingSince: each host's dbFrom
// its first stored point (firsts: the vnode's, localhost's; rrdhost-status.c:121-122), and the vnode's InSince the
// agent's start (a local host online, rrdhost-status.c:175-177) as localhost's.
func setbVnodeFnFacts(firsts [2]int64, started [2]int64) func(Value) error {
	return func(v Value) error {
		for k, name := range []string{vnodeName, parentIdentity.Hostname} {
			r := setbRow(v, name)
			if r < 0 {
				return fmt.Errorf("no row of %s", name)
			}
			cells, err := setbCells(v, r, "dbFrom", "InSince")
			if err != nil {
				return err
			}
			if cells["dbFrom"] != firsts[k]*1000 {
				return fmt.Errorf("row %d (%s): dbFrom %d, its first stored point is %d", r, name, cells["dbFrom"],
					firsts[k])
			}
			if !setbWithin(cells["InSince"], started) {
				return fmt.Errorf("row %d (%s): InSince %d is no second of the agent's start %v", r, name,
					cells["InSince"], started)
			}
		}
		return nil
	}
}

// setbVnodeAsk is the vnode table's comparison, given each side's firsts and start windows.
func setbVnodeAsk(firsts [2][2]int64, sides [2]niSide) fnStreamingAsk {
	return fnStreamingAsk{target: "/api/v1/function?function=netdata-streaming", guard: setbVnodeGuard,
		settle: dashSettle, facts: func(i int, v Value, _ [2]int64) error {
			return setbVnodeFnFacts(firsts[i], sides[i].started)(v)
		}}
}

// setbLocalPoll judges one raw answer of setbLocalQueryable's poll: the db of the first instance of the node named
// name (its status and retention, or what is wrong with the answer), and whether it is settled: `online`, its first
// stored point a second before its last time (an online host's last time is the walk's clock, rrdhost.h:618).
// Anything else is asked again.
func setbLocalPoll(answer []byte, name string) (got string, done bool) {
	v, err := ParseJSON(httpBody(answer))
	if err != nil {
		return err.Error(), false
	}
	nodes, _ := dashMember(v, "nodes")
	for _, node := range nodes.Items {
		if nm, err := dashMember(node, "nm"); err != nil || nm.Kind != KindString || nm.Text != name {
			continue
		}
		db, err := dashAt(node, "instances", "[0]", "db")
		if err != nil {
			return err.Error(), false
		}
		status, _ := dashMember(db, "status")
		first, _ := dashMember(db, "first_time")
		last, _ := dashMember(db, "last_time")
		f, _ := strconv.ParseInt(first.Text, 10, 64)
		l, _ := strconv.ParseInt(last.Text, 10, 64)
		return fmt.Sprintf("%s from %s to %s", status, first, last),
			status.Kind == KindString && status.Text == "online" && 0 < f && f < l
	}
	return "no node " + name, false
}

// setbLocalWait bounds setbLocalQueryable's wait for one side, as waitQueryable's for the vnode.
const setbLocalWait = 10 * time.Second

// setbLocalQueryable waits until each side's localhost reads db `online` in its node instances, with a second of
// retention (setbLocalPoll). waitQueryable waits for the vnode alone, and localhost's chart is collected after the
// vnode's: C reads a host's db `initializing` (and its ingestion `initializing`, its liveness `stale`) until the
// host's cached retention is set (rrdhost-status.c:122-132, :172-173, :387-390; rrdhost.h:607-619). Its contexts
// worker sets it when it post-processes the host's collected context on its own heartbeat, after the instance's and
// context's collected counts (rrdcontext-worker.c:528, :799, then :811 and :975-981; or :127-137 for every context at
// once), so a host read online also reads them. Within the second of its first stored point the admin's table
// writes the host's retention duration null (function-netdata-streaming.c:161-167). SA-B's probe p5 caught the
// younger side's localhost `initializing` at the first ask in both runs; SA-B3's probe p1, once it waited for
// `online` alone, the table's duration null at its first ask in all three runs. The polls are tagged harness=wait.
// The oracle's failure ends the case; a candidate's is reported.
func setbLocalQueryable(t *testing.T, p *Pair) {
	t.Helper()
	target := "/api/v3/node_instances?scope_nodes=" + parentIdentity.Hostname + "&harness=wait"
	for _, side := range p.Each() {
		got := ""
		if pollUntil(setbLocalWait, func() bool {
			b, err := v2Exchange(side.Daemon.Addr, v2Req{target: target})
			if err != nil {
				got = err.Error()
				return false
			}
			done := false
			got, done = setbLocalPoll(b, parentIdentity.Hostname)
			return done
		}) {
			continue
		}
		if side.Role == Oracle {
			t.Fatalf("oracle: localhost's db is %s after %v, not online with a second of retention", got, setbLocalWait)
		}
		t.Errorf("candidate: localhost's db is %s after %v, not online with a second of retention", got, setbLocalWait)
	}
}

// setbVnode (TestNodeInstancesSetB/vnode; plan 4.2 `vnode`): plugins.vnodes' `define` scenario with fn.http's bearer
// tokens (a run of its own: the check's records stay its own): the fake plugin defines the vnode, collects three
// blocks into it, then two into localhost, and waits; once each side's vnode is queryable (waitQueryable) and its
// localhost too (setbLocalQueryable), the node instances of every host and the admin's table.
func setbVnode(t *testing.T) {
	define := plugin.Step{Emit: vnodeDefine(vnodeGUID, vnodeName, vnodeLabels...)}
	runPluginCases(t, map[string]pluginCase{"define": {
		prepare: fnWriteTokens,
		sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
			define,
			{Collect: &plugin.Collect{Chart: "difftest.vn", Dims: []string{"x"}, N: 3}},
			{Emit: "HOST localhost\n"},
			{Collect: &plugin.Collect{Chart: "difftest.lo", Dims: []string{"x"}, N: 2}},
			{WaitFile: "release-1"},
			{Hang: true},
		}}}},
		play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
			waitPlugin(t, p, ls, "the plugin did not collect and wait", waiting(1, "release-1"))
			waitQueryable(t, p, 10*time.Second)
			setbLocalQueryable(t, p)
			firsts := setbVnodeFirsts(t, ls)
			sides := niSides(p, [2][2]int64{})
			r := v2Req{name: "v3-ni", target: "/api/v3/node_instances", status: "200", guard: setbVnodeFacts()}
			t.Run(r.name, func(t *testing.T) { compareV2(t, p, r, setbVnodeFamily(sides, firsts)) })
			fnStreamingCompareWith(t, p, setbVnodeAsk(firsts, sides))
		},
		guard: func(*testing.T, [][]plugin.Record, map[string][]string) {},
	}})
}

// ---------------------------------------------------------------------------------------------------------------
// fn.stream's calls topology (fnStreamNodeInstances)

// fnStreamNIFamily compares the node instances of fn.stream's parents: nodeInstancesFamily with each side's C child's
// window (sides: from the child's launch to the comparison's start) as the connections' window (CONNECTED: the child's
// and its vnode's receivers both attach in it), and a database's first time in that window CHILD-FIRST: the child's
// first stored point, replicated from its own collection, and its vnode's.
func fnStreamNIFamily(sides [2]niSide) v2Family {
	fam := nodeInstancesFamily(sides)
	outside := fam.render
	fam.render = func(i int, flight [2]int64, body []byte) []byte {
		return setbFirstRe.ReplaceAllFunc(outside(i, flight, body), func(m []byte) []byte {
			g := setbFirstRe.FindSubmatch(m)
			if n, err := strconv.ParseInt(string(g[2]), 10, 64); err == nil && n >= sides[i].opened[0] &&
				n <= sides[i].opened[1] {
				return []byte(`"first_time":` + string(g[1]) + `"CHILD-FIRST"`)
			}
			return m
		})
	}
	return fam
}

// fnStreamNIFacts are the facts of `/api/v3/node_instances` on a parent of fn.stream's `calls` topology once its cases
// played (C in SA-B's probe p1): three hosts in creation order:
//   - localhost, initializing with the pulse off (niLocalFacts);
//   - the C child (stream.rchild's identity): its database in the stream's memory mode with its metrics (C against C
//     equal; compared), from its first replicated point (CHILD-FIRST) until now; its ingestion a child's online on its
//     receiver, its first connection, one hop, with its source (the negotiated names of a C child's sender, compression
//     off: the parents' `enable compression` is the launcher's); its functions: the built-ins its re-list carries and
//     its plugins' methods, `difftest-gone` kept after its plugin exited (no FUNCTION_DEL), the restricted left out
//     (nrpc-catalog.c:140-177, :38-48); the capabilities of a child that streams functions and whose dyncfg is online
//     (aclk_capas.c:41-42, :53); its dyncfg online: DynCfg is available for it (rrdhost-status.c:404-405; the child
//     registered its `config` method, daemon/dyncfg/dyncfg.c:531-536);
//   - the child's vnode on a receiver of its own: a child's ingestion, two hops (the C child's one and its own),
//     online, its plugin's method alone, dyncfg unavailable (it has no `config`);
//   - the agent's nodes: three, two received.
var fnStreamNIFacts = func() func(Value) error {
	inst := func(i int, key string) []string {
		return []string{"nodes", "[" + strconv.Itoa(i) + "]", "instances", "[0]", key}
	}
	keys := "st db ingest ml health functions capabilities dyncfg"
	ingestKeys := "id hops type status since age metrics instances contexts source"
	db := func(i int) []dashFact {
		return slices.Concat(dashMembers(inst(i, "db"), "status", `"online"`, "liveness", `"live"`, "mode", `"ram"`,
			"first_time", `"CHILD-FIRST"`, "last_time", `"NOW"`), []dashFact{dashAbove(0, append(inst(i, "db"), "metrics")...)})
	}
	source := `{"local":"[127.0.0.1]:LISTEN","remote":"[127.0.0.1]:PEER","capabilities":["VCAPS","HLABELS","CLAIM",` +
		`"CLABELS","FUNCTIONS","FUNCDEL","REPLICATION","BINARY","INTERPOLATED","IEEE754","DYNCFG","SLOTS","PROGRESS",` +
		`"NODEID","PATHS","FLOATBASELINE"]}`
	return dashGuard([]dashFact{niMembers, dashAbsent("nodes", "[3]"),
		dashKeys(keys, "nodes", "[0]", "instances", "[0]"), dashKeys(keys, "nodes", "[1]", "instances", "[0]"),
		dashKeys(keys, "nodes", "[2]", "instances", "[0]"),
		dashKeys(ingestKeys, inst(1, "ingest")...), dashKeys(ingestKeys, inst(2, "ingest")...),
		dashKeys("netdata-streaming topology:streaming netdata-api-calls netdata-metrics-cardinality difftest-open "+
			"difftest-fn difftest-gone", inst(1, "functions")...),
		dashKeys("difftest-vfn", inst(2, "functions")...)},
		niLocalFacts(niShort),
		dashNode(1, 1, rchildGUID, rchildHostname), db(1),
		dashMembers(inst(1, "ingest"), "id", "1", "hops", "1", "type", `"child"`, "status", `"online"`, "since",
			`"CONNECTED"`, "age", `"NOW-SINCE"`, "source", source),
		dashMembers([]string{"nodes", "[1]", "instances", "[0]"}, "capabilities",
			nodeCapsOf(capFuncsOn, capHealthOff, capDyncfgOn), "dyncfg", `{"status":"online"}`),
		dashNode(2, 2, rvGUID, rvName), db(2),
		dashMembers(inst(2, "ingest"), "id", "1", "hops", "2", "type", `"child"`, "status", `"online"`, "since",
			`"CONNECTED"`, "age", `"NOW-SINCE"`, "metrics", "1", "source", source),
		dashMembers([]string{"nodes", "[2]", "instances", "[0]"}, "capabilities",
			nodeCapsOf(capFuncsOn, capHealthOff, capDyncfgOff), "dyncfg", `{"status":"unavailable"}`),
		niAgentFactsOf(parentIdentity.MachineGUID, `{"total":3,"receiving":2,"sending":0,"archived":0}`))
}()

// fnStreamNodeInstances compares `/api/v3/node_instances` on fn.stream's two parents once the `calls` cases played
// (plan 4.2 `fn.stream`; 7.3 question 8): fnStreamNIFamily, each side's connections' window its C child's launch to
// now, fnStreamNIFacts.
func fnStreamNodeInstances(t *testing.T, p *Pair, sides [2]*fnStreamSide) {
	t.Helper()
	now := time.Now().Unix()
	ni := niSides(p, [2][2]int64{})
	for i := range ni {
		ni[i].opened = [2]int64{sides[i].child.LaunchStartedAt.Unix(), now}
	}
	compareV2(t, p, v2Req{name: "v3-ni", target: "/api/v3/node_instances", status: "200", guard: fnStreamNIFacts},
		fnStreamNIFamily(ni))
}

// ---------------------------------------------------------------------------------------------------------------
// archived: a host loaded from a dbengine database, with its retention (setbArchived)

// setbArchivedSince holds an archived host's ingestion start (at path ingest) to its database's last time (at db),
// both numbers, after its first: an archived host begins at its data's end (rrdhost-status.c:197-198), where one
// without retention begins at the agent's start (:202, niArchivedFacts).
func setbArchivedSince(db, ingest []string) dashFact {
	return func(v Value) error {
		var n [3]int64
		for k, at := range [][]string{append(slices.Clone(db), "first_time"), append(slices.Clone(db), "last_time"),
			append(slices.Clone(ingest), "since")} {
			x, err := dashAt(v, at...)
			if err != nil {
				return err
			}
			if n[k], err = strconv.ParseInt(x.Text, 10, 64); err != nil || x.Kind != KindNumber {
				return fmt.Errorf("%s is %s, want a number", dashPath(at), x)
			}
		}
		if n[2] != n[1] || n[0] <= 0 || n[0] >= n[1] {
			return fmt.Errorf("%s: first_time %d, last_time %d, the ingestion's since %d: want the since the data's end",
				dashPath(ingest), n[0], n[1], n[2])
		}
		return nil
	}
}

// setbArchivedFacts are the facts of `/api/v3/node_instances?scope_nodes=parity-child` on setbArchived's pair (C in
// SA-B's probe p4): the child alone, numbered 0, a host loaded from the database that never connected in this run:
// its node and instance as a host's without functions (niHostInstance: no `functions`, an archived host gets no
// function registry, database/rrdhost.c:635-639, nrpc/nrpc-catalog.c:205-227; no `stream`); its database queryable,
// a dbengine host's contexts and retention loaded (rrdhost-status.c:124-132; rrdcontext-loading.c:171-176), but stale
// (:385-390), its two charts' one metric each; its ingestion archived, with no connection (:188-189, :232, :234), the
// database's one hop, begun at the data's end (setbArchivedSince), nothing collected; the agent's nodes: two, the
// child archived (rrd-metadata.c:35-44).
func setbArchivedFacts() func(Value) error {
	_, db, ingest := niChildPaths(0)
	return dashGuard([]dashFact{niMembers, dashAbsent("nodes", "[1]")},
		niHostInstance(niShort, 0, 0, childHost.MachineGUID, childHost.Hostname, false),
		dashMembers(db, "status", `"online"`, "liveness", `"stale"`, "mode", `"dbengine"`, "metrics", "2",
			"instances", "2", "contexts", "2"),
		[]dashFact{dashKeys("id hops type status since age metrics instances contexts", ingest...),
			setbArchivedSince(db, ingest)},
		dashMembers(ingest, "id", "0", "hops", "1", "type", `"archived"`, "status", `"archived"`, "age",
			`"NOW-SINCE"`, "metrics", "0", "instances", "0", "contexts", "0"),
		niAgentFactsOf(parentIdentity.MachineGUID, `{"total":2,"receiving":0,"sending":0,"archived":1}`))
}

// setbArchivedRow is setbArchived's node-instance request: the archived child alone (localhost's pulse counts out of
// the scope; the agent block's stay compared, as niArchivedRow's on a pair whose pulse is on).
func setbArchivedRow() v2Req {
	return v2Req{name: "v3-ni", target: "/api/v3/node_instances?scope_nodes=" + childHost.Hostname, status: "200",
		guard: setbArchivedFacts()}
}

// setbArchivedGuard is the guard of the oracle's table on setbArchived's pair: localhost online (its pulse collects:
// rrdhost-status.c:175-177) without a sender, of severity normal; then the archived child, of severity critical
// (function-netdata-streaming.c:107-115), without a sender, no connection, its inbound reason the stored 0's
// (:199-204: NEVER CONNECTED), its hops the database's one and the sender's one more (rrdhost-status.c:103-112,
// :246), no local address (function-netdata-streaming.c:212: neither local nor received), its two charts' metric
// each in the database and none collected.
var setbArchivedGuard = fnStreamingRows(
	fnStreamingRow{"Node": strconv.Quote(parentIdentity.Hostname), "rowOptions": `{"severity":"normal"}`,
		"InReason": `"LOCALHOST"`, "InStatus": `"online"`, "OutStatus": `"disabled"`},
	fnStreamingRow{"Node": strconv.Quote(childHost.Hostname), "rowOptions": `{"severity":"critical"}`,
		"Ephemerality": `"permanent"`, "InStatus": `"archived"`, "OutStatus": `"disabled"`, "InConnections": "0",
		"InReason": `"NEVER CONNECTED"`, "InHops": "1", "OutHops": "2", "InLocalIP": `""`, "dbMetrics": "2",
		"dbInstances": "2", "dbContexts": "2", "CollectedMetrics": "0"},
)

// setbArchivedFnFacts holds a side's table besides fnStreamingFacts and fnStreamingSince: the archived child's InSince
// (masked, function-netdata-streaming.c:185-187) its retention's end, dbTo (rrdhost-status.c:197-198; an offline
// host's retention stays compared, fnStreamingMasks), after its start.
func setbArchivedFnFacts(v Value) error {
	r := setbRow(v, childHost.Hostname)
	if r < 0 {
		return fmt.Errorf("no row of %s", childHost.Hostname)
	}
	cells, err := setbCells(v, r, "InSince", "dbFrom", "dbTo")
	if err != nil {
		return err
	}
	if cells["InSince"] != cells["dbTo"] || cells["dbFrom"] <= 0 || cells["dbFrom"] >= cells["dbTo"] {
		return fmt.Errorf("row %d: InSince %d, dbFrom %d, dbTo %d: want the since the retention's end", r,
			cells["InSince"], cells["dbFrom"], cells["dbTo"])
	}
	return nil
}

// setbArchivedAsk is setbArchived's table comparison.
var setbArchivedAsk = fnStreamingAsk{target: "/api/v1/function?function=netdata-streaming", guard: setbArchivedGuard,
	settle: dashSettle, facts: func(_ int, v Value, _ [2]int64) error { return setbArchivedFnFacts(v) }}

// setbArchived (TestNodeInstancesSetB/archived; the close's readers of commits 11 and 12: an archived host's `since` at
// its data's end, an archived host's row of the table): two agents on copies of one C-written dbengine cache that
// knows the fixture child (seedFromOracle, as sqlite.archived-hosts-dbengine's), the pulse on, with fn.http's bearer
// tokens, as SA-B's probe p4 ran them; once both start windows are over (niReady) and three seconds on, the child's
// node instances and the admin's table. A pair of its own: TestArchivedHostsDbengine compares the daemon log whole
// afterwards, which a request may change.
func setbArchived(t *testing.T) {
	seed := seedFromOracle(t, parentIdentity, "dbengine")
	p := startPairWith(t, daemon.Options{StorageTiers: 1, StreamMemoryMode: "dbengine"}, parentIdentity, binaries(t),
		[2]string{seed, seed}, [2]Role{Oracle, Candidate}, fnWriteTokens)
	niReady(p)
	time.Sleep(3 * time.Second)
	r := setbArchivedRow()
	t.Run(r.name, func(t *testing.T) { compareV2(t, p, r, nodeInstancesFamily(niSides(p, [2][2]int64{}))) })
	fnStreamingCompareWith(t, p, setbArchivedAsk)
}

// ---------------------------------------------------------------------------------------------------------------
// claim: a vnode that claimed a child's host, then let it go (setbClaim)

// setbClaimStages are the claim's stages, in order, and what C printed of the vnode in each (SA-B's probe p4):
//   - claimed: the plugin defined the vnode while a child streamed its GUID: the receiver stopped with -40
//     (pluginsd_parser.c:240), the host virtual and collected: online since the agent started (a local host,
//     rrdhost-status.c:175-177), its connection count the child's 1 (:234), the child's chart and the plugin's;
//   - released: the plugin ended, which clears the host's VIRTUAL and COLLECTOR_ONLINE flags (pluginsd_parser.c:
//     1520-1524): neither local nor collected, with one connection: offline (rrdhost-status.c:191), of type archived
//     (:232), begun at the receiver's detach (:169; stream-receiver.c:1502), its reason the claim's
//     (`DISCONNECTED LOCAL VNODE CLAIMED`, stream-handshake.c:31), its collected counts still 1.
var setbClaimStages = []string{"claimed", "released"}

// setbClaimSide is what the claim rows take from one side's own run: niSide's windows (gone: from the claim's release
// to the child's eviction, the seconds its receiver's detach was stamped in) and the seconds vnodeChild took its
// points' clock in (child).
type setbClaimSide struct {
	niSide
	child [2]int64
}

// childFirst holds when the second sec is the vnode's database start: its child's first point's (vnodeChild's
// three points end a second before its clock, the first covering the second before its own: C in SA-B's probe p4,
// 1791647521 for a clock of 1791647525).
func (s setbClaimSide) childFirst(sec int64) bool {
	return s.child[1] > 0 && sec >= s.child[0]-4 && sec <= s.child[1]-4
}

// setbClaimFamily compares the vnode's node instances: nodeInstancesFamily of the sides' windows (START the agent's,
// GONE the detach's), and the database's first time CHILD-FIRST where it is the side's child's first point
// (childFirst).
func setbClaimFamily(sides [2]setbClaimSide) v2Family {
	fam := nodeInstancesFamily([2]niSide{sides[0].niSide, sides[1].niSide})
	outside := fam.render
	fam.render = func(i int, flight [2]int64, body []byte) []byte {
		return setbFirstRe.ReplaceAllFunc(outside(i, flight, body), func(m []byte) []byte {
			g := setbFirstRe.FindSubmatch(m)
			if n, err := strconv.ParseInt(string(g[2]), 10, 64); err == nil && sides[i].childFirst(n) {
				return []byte(`"first_time":` + string(g[1]) + `"CHILD-FIRST"`)
			}
			return m
		})
	}
	return fam
}

// setbStoredEnd holds the number at path: a database's last time that is its stored end, not the walk's clock (NOW).
func setbStoredEnd(path ...string) dashFact {
	return func(v Value) error {
		x, err := dashAt(v, path...)
		if err != nil {
			return err
		}
		if _, perr := strconv.ParseInt(x.Text, 10, 64); perr != nil || x.Kind != KindNumber {
			return fmt.Errorf("%s is %s, want the stored end, a number", dashPath(path), x)
		}
		return nil
	}
}

// setbClaimFacts are the facts of `/api/v3/node_instances?scope_nodes=parity-vnode` at stage (setbClaimStages): the
// vnode alone, numbered 0, a host that keeps its function registry, with none (niHostInstance); its database in the
// child's memory mode, the child's chart and the plugin's, from the child's first point (CHILD-FIRST), live until now
// while it is collected, then stale until its stored end; its ingestion one connection, one hop, its collected counts
// 1: virtual and online since the agent's start (START), then archived and offline since the detach (GONE); the
// agent's nodes: two, the vnode received (online, not localhost), then archived (rrd-metadata.c:35-44).
func setbClaimFacts(stage string) func(Value) error {
	_, db, ingest := niChildPaths(0)
	liveness, last, typ, status, since := `"live"`, []dashFact{dashIs(`"NOW"`, append(slices.Clone(db), "last_time")...)},
		`"virtual"`, `"online"`, `"START"`
	nodes := `{"total":2,"receiving":1,"sending":0,"archived":0}`
	if stage == "released" {
		liveness, last, typ, status, since = `"stale"`, []dashFact{setbStoredEnd(append(slices.Clone(db), "last_time")...)},
			`"archived"`, `"offline"`, `"GONE"`
		nodes = `{"total":2,"receiving":0,"sending":0,"archived":1}`
	}
	return dashGuard([]dashFact{niMembers, dashAbsent("nodes", "[1]")},
		niHostInstance(niShort, 0, 0, vnodeGUID, vnodeName, true),
		dashMembers(db, "status", `"online"`, "liveness", liveness, "mode", `"ram"`, "first_time", `"CHILD-FIRST"`,
			"metrics", "2", "instances", "2", "contexts", "2"), last,
		[]dashFact{dashKeys("id hops type status since age metrics instances contexts", ingest...)},
		dashMembers(ingest, "id", "1", "hops", "1", "type", typ, "status", status, "since", since, "age",
			`"NOW-SINCE"`, "metrics", "1", "instances", "1", "contexts", "1"),
		niAgentFactsOf(parentIdentity.MachineGUID, nodes))
}

// setbClaimRow is the vnode's node-instance request at stage.
func setbClaimRow(stage string) v2Req {
	return v2Req{name: "v3-ni", target: "/api/v3/node_instances?scope_nodes=" + vnodeName, status: "200",
		guard: setbClaimFacts(stage)}
}

// setbClaimGuard is the guard of the oracle's table at stage: localhost initializing (nothing collected into it,
// the pulse off) without a sender, of severity normal; then the vnode without a sender, its one connection, its hops
// one in and one more out (rrdhost-status.c:105, :246), its two charts' metric each in the database, one collected:
// online, of severity normal, its reason VIRTUAL NODE and its local address `localhost` (function-netdata-streaming.c:
// 199-204, :212) while it is virtual; then offline, of severity critical (:107-115), its reason the claim's stored -40
// and no local address.
func setbClaimGuard(stage string) func(Value) error {
	vnode := fnStreamingRow{"Node": strconv.Quote(vnodeName), "rowOptions": `{"severity":"normal"}`,
		"InStatus": `"online"`, "OutStatus": `"disabled"`, "InConnections": "1", "InReason": `"VIRTUAL NODE"`,
		"InHops": "1", "OutHops": "2", "InLocalIP": `"localhost"`, "dbMetrics": "2", "CollectedMetrics": "1"}
	if stage == "released" {
		vnode["rowOptions"], vnode["InStatus"] = `{"severity":"critical"}`, `"offline"`
		vnode["InReason"], vnode["InLocalIP"] = `"DISCONNECTED LOCAL VNODE CLAIMED"`, `""`
	}
	return fnStreamingRows(fnStreamingRow{"Node": strconv.Quote(parentIdentity.Hostname),
		"rowOptions": `{"severity":"normal"}`, "InReason": `"LOCALHOST"`, "InStatus": `"initializing"`,
		"OutStatus": `"disabled"`, "dbMetrics": "0"}, vnode)
}

// setbClaimFnFacts hold side's table at stage besides fnStreamingFacts and fnStreamingSince (which holds the released
// vnode's InSince to the side's detach, ask.left): the vnode's dbFrom its child's first point (childFirst; masked
// while the vnode is online), and while it is virtual its InSince the agent's start (rrdhost-status.c:175-177).
func setbClaimFnFacts(stage string, side setbClaimSide) func(Value) error {
	return func(v Value) error {
		r := setbRow(v, vnodeName)
		if r < 0 {
			return fmt.Errorf("no row of %s", vnodeName)
		}
		cells, err := setbCells(v, r, "dbFrom", "InSince")
		if err != nil {
			return err
		}
		if cells["dbFrom"]%1000 != 0 || !side.childFirst(cells["dbFrom"]/1000) {
			return fmt.Errorf("row %d: dbFrom %d is not the child's first point (its clock %v)", r, cells["dbFrom"],
				side.child)
		}
		if stage == "claimed" && !setbWithin(cells["InSince"], side.started) {
			return fmt.Errorf("row %d: InSince %d is no second of the agent's start %v", r, cells["InSince"],
				side.started)
		}
		return nil
	}
}

// setbClaimAsk is the claim table's comparison at stage, given each side's windows.
func setbClaimAsk(stage string, sides [2]setbClaimSide) fnStreamingAsk {
	ask := fnStreamingAsk{target: "/api/v1/function?function=netdata-streaming", guard: setbClaimGuard(stage),
		settle: dashSettle, facts: func(i int, v Value, _ [2]int64) error { return setbClaimFnFacts(stage, sides[i])(v) }}
	if stage == "released" {
		for i := range sides {
			ask.left[i] = map[string][2]int64{vnodeName: sides[i].gone}
		}
	}
	return ask
}

// setbClaimCompare runs stage's rows on p.
func setbClaimCompare(t *testing.T, p *Pair, stage string, sides [2]setbClaimSide) {
	t.Run(stage, func(t *testing.T) {
		r := setbClaimRow(stage)
		t.Run(r.name, func(t *testing.T) { compareV2(t, p, r, setbClaimFamily(sides)) })
		fnStreamingCompareWith(t, p, setbClaimAsk(stage, sides))
	})
}

// setbClaim (TestNodeInstancesSetB/claim; commit 12's block: the vnode claim's -40 as InReason), as SA-B's probe p4
// played it: plugins.vnodes' claim-evicts topology with fn.http's bearer tokens, the plugin waiting before its define;
// once both start windows are over (niReady), a child streams the vnode's GUID into each side (vnodeChild, both in one
// second: the vnode's database starts at its first point, and an offline host's retention is compared), then the
// plugin defines the vnode (released at mid-second, so both plugins' blocks fall in the same seconds), which evicts the
// child; once the plugin collected, `claimed`; then the plugin exits (its next start hangs, nothing defines the vnode
// again), `released`. The plugin's records are not guarded: claim-evicts guards this topology's.
func setbClaim(t *testing.T) {
	define := plugin.Step{Emit: vnodeDefine(vnodeGUID, vnodeName, vnodeLabels...)}
	runPluginCases(t, map[string]pluginCase{"claim": {
		prepare: fnWriteTokens,
		sc: plugin.Scenario{Starts: []plugin.Start{
			{Steps: []plugin.Step{{WaitFile: "release-1"}, define,
				{Collect: &plugin.Collect{Chart: "difftest.vc", Dims: []string{"x"}, N: 2}},
				{WaitFile: "release-2"}, {Exit: plugin.ExitCode(0)}}},
			{Steps: []plugin.Step{{Hang: true}}},
		}},
		play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
			waitPlugin(t, p, ls, "the plugin did not start", waiting(1, "release-1"))
			niReady(p)
			var sides [2]setbClaimSide
			for i, s := range niSides(p, [2][2]int64{}) {
				sides[i].niSide = s
			}
			time.Sleep(time.Until(time.Unix(time.Now().Unix()+1, 50e6)))
			var conns [2]*stream.Conn
			for i, side := range p.Each() {
				from := time.Now().Unix()
				conns[i] = vnodeChild(t, side.Daemon)
				sides[i].child = [2]int64{from, time.Now().Unix()}
			}
			for _, side := range p.Each() {
				if !pollUntil(30*time.Second, func() bool { ok, _ := hasReceiver(side.Daemon.Addr, vnodeGUID); return ok }) {
					t.Fatalf("%s: no receiver for the vnode's GUID", side.Role)
				}
			}
			time.Sleep(time.Until(time.Unix(time.Now().Unix()+1, 500e6)))
			from := time.Now().Unix()
			releaseAll(t, ls, "release-1")
			for i, side := range p.Each() {
				if err := waitEvicted(conns[i], 20*time.Second); err != nil {
					t.Fatalf("%s: the child was not evicted: %v", side.Role, err)
				}
				sides[i].gone = [2]int64{from, time.Now().Unix()}
			}
			waitPlugin(t, p, ls, "the plugin did not collect", waiting(1, "release-2"))
			waitIngest(t, p, "virtual", 10*time.Second)
			waitQueryable(t, p, 10*time.Second)
			time.Sleep(2 * time.Second)
			setbClaimCompare(t, p, "claimed", sides)
			releaseAll(t, ls, "release-2")
			waitPlugin(t, p, ls, "the plugin did not end", startEnded(1))
			time.Sleep(2 * time.Second)
			setbClaimCompare(t, p, "released", sides)
		},
		guard: func(*testing.T, [][]plugin.Record, map[string][]string) {},
	}})
}

// TestNodeInstancesSetB compares `/api/v3/node_instances` and the admin's netdata-streaming table in set B's states
// (check `api.v2-node-instances-setb`, H41; D241 F3, D248): a child that replicates (`replicating`), a proxied child
// that replicates up the stream (`relay`), a vnode beside a collecting localhost (`vnode`), a dbengine host loaded
// from the database with its retention (`archived`), a vnode that claimed a child's host and let it go (`claim`).
// fn.stream's row is fnStream's `calls` topology's (fnStreamNodeInstances).
func TestNodeInstancesSetB(t *testing.T) {
	t.Run("replicating", setbRepl)
	t.Run("relay", setbRelay)
	t.Run("vnode", setbVnode)
	t.Run("archived", setbArchived)
	t.Run("claim", setbClaim)
}
