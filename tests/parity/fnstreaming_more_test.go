// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"net"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/plugin"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The rows of `netdata-streaming` beside `fn.netdata-streaming`'s `calls` comparison (milestone 10 commit 12, D241
// F3): the admin's table of a standalone agent in `fn.builtins` (fnStreamingView: `access`'s two calls, and
// `streaming-info`), and `fn.netdata-streaming`'s `gone` topology, the only observer of a receiver's stored reason
// (fnStreamingGone).

// fnStreamingSince holds one side's table to the seconds C takes its times from, given the seconds the agent started
// in (its launch and niStartSlack seconds after it) and, by hostname, the seconds each host that left was
// disconnected in (from the second its connection was ended in to the answer that showed its detach's stamp,
// niGoneWindows):
//   - localhost's InSince and the OutSince of every host without a sender (OutStatus `disabled`) are the agent's
//     start, one second of the window for all of them (netdata_start_time, daemon/main.c:338): localhost's
//     ingestion has no connection time (rrdhost-status.c:175-177, :202) and a host without a sender no state time
//     (:244-248, :289-290);
//   - a host that left reads the second its receiver ended (`last_disconnected`, :164, :169, stream-receiver.c:1502).
func fnStreamingSince(v Value, started [2]int64, left map[string][2]int64) error {
	data, err := dashMember(v, "data")
	if err != nil {
		return err
	}
	start, starts := int64(0), 0
	seen := map[string]bool{}
	for r := range data.Items {
		cells := map[string]Value{}
		for _, col := range []string{"Node", "InReason", "OutStatus", "InSince", "OutSince"} {
			if cells[col], err = fnStreamingCell(v, r, col); err != nil {
				return err
			}
		}
		node := cells["Node"].Text
		// second is col's cell as the second it is the milliseconds of, inside window
		second := func(col string, window [2]int64, what string) (int64, error) {
			ms, null, err := fnStreamingWhole(cells[col])
			if err != nil || null || ms%1000 != 0 || ms/1000 < window[0] || ms/1000 > window[1] {
				return 0, fmt.Errorf("row %d (%s): %s is %s, want the milliseconds of a second of %s [%d, %d]", r, node,
					col, cells[col], what, window[0], window[1])
			}
			return ms / 1000, nil
		}
		var fromStart []string
		if cells["InReason"].Text == "LOCALHOST" {
			fromStart = append(fromStart, "InSince")
		}
		if cells["OutStatus"].Text == "disabled" {
			fromStart = append(fromStart, "OutSince")
		}
		for _, col := range fromStart {
			s, err := second(col, started, "the agent's start")
			if err != nil {
				return err
			}
			if starts > 0 && s != start {
				return fmt.Errorf("row %d (%s): %s is the second %d, another cell read the start as %d", r, node, col, s,
					start)
			}
			start, starts = s, starts+1
		}
		if w, ok := left[node]; ok {
			if _, err := second("InSince", w, "its disconnection"); err != nil {
				return err
			}
			seen[node] = true
		}
	}
	if starts == 0 {
		return fmt.Errorf("no cell reads the agent's start")
	}
	for node := range left {
		if !seen[node] {
			return fmt.Errorf("no row of %s, which left", node)
		}
	}
	return nil
}

// ---------------------------------------------------------------------------------------------------------------
// The standalone table (fn.builtins)

// fnStreamingStandalone is the guard of the oracle's table on fn.builtins' pair: localhost alone, initializing (the
// fake plugin collects nothing and PULSE is off: rrdhost-status.c:124-130, :172-173), without a sender (:244-248), of
// severity normal (function-netdata-streaming.c:107-138).
var fnStreamingStandalone = fnStreamingRows(fnStreamingRow{"Node": strconv.Quote(parentIdentity.Hostname),
	"rowOptions": `{"severity":"normal"}`, "InReason": `"LOCALHOST"`, "InStatus": `"initializing"`,
	"OutStatus": `"disabled"`})

// fnStreamingSide is what one side's table is held to besides its cells (fnStreamingRender): the port its agent
// listens on, the seconds its request was in flight, the seconds the agent started in.
type fnStreamingSide struct {
	port    string
	flight  [2]int64
	started [2]int64
}

// fnStreamingRender is one side's answer of the admin's netdata-streaming call as a case's exchange compares it: the
// head through fnHTTPMask with its length as N (held to the body's own, rawLength), then the body's bytes with the
// volatile columns' numbers other than 0 written `<masked:NAME>` (fnStreamingMasks: the cells
// fnStreamingCompareWith masks) and the expiry as its distance from Date (fnBuiltinsExpires). Every other byte is
// compared, the layout and the strings' escapes with them. An answer that is not a 200 with a JSON body is
// fnHTTPMask's whole. problems are the side's table against what C fixes (fnStreamingFacts, fnStreamingSince) and,
// when guard is set (the oracle's), the guard.
func fnStreamingRender(resp []byte, side fnStreamingSide, guard func(Value) error) (string, []string) {
	var problems []string
	head, body, ok := bytes.Cut(resp, []byte("\r\n\r\n"))
	if !ok || fnStatusLine(resp) != "HTTP/1.1 200 OK" {
		if guard != nil {
			problems = append(problems, "answered "+strconv.Quote(truncateBytes(resp)))
		}
		return fnHTTPMask(resp), problems
	}
	_, doc, err := fnStreamingDoc(resp)
	if err != nil {
		return fnHTTPMask(resp), append(problems, err.Error())
	}
	if guard != nil {
		if err := guard(doc); err != nil {
			problems = append(problems, "guard: "+err.Error())
		}
	}
	if err := rawLength(resp); err != nil {
		problems = append(problems, err.Error())
	}
	if err := fnStreamingFacts(doc, side.port, side.flight); err != nil {
		problems = append(problems, err.Error())
	}
	if err := fnStreamingSince(doc, side.started, nil); err != nil {
		problems = append(problems, err.Error())
	}
	text, err := fnStreamingMaskText(body, fnStreamingMasks(doc, fnStreamingVolatile))
	if err != nil {
		return fnHTTPMask(resp), append(problems, err.Error())
	}
	h := contentLengthRe.ReplaceAllString(fnHTTPMask(slices.Concat(head, []byte("\r\n\r\n"))), "Content-Length: N")
	return h + string(fnBuiltinsExpires(text, fnHTTPDate(resp))), problems
}

// fnStreamingMaskText is body with the value at each mask's path (object keys and `[i]` items, as fnStreamingMasks
// writes them) written `<masked:REASON>`, every other byte kept.
func fnStreamingMaskText(body []byte, masks []Mask) ([]byte, error) {
	type cut struct {
		start, end int
		with       string
	}
	var cuts []cut
	for _, m := range masks {
		start, end, err := jsonSpanAt(body, strings.Split(m.Pattern, "."))
		if err != nil {
			return nil, fmt.Errorf("harness: %s: %v", m.Pattern, err)
		}
		cuts = append(cuts, cut{start, end, "<masked:" + m.Reason + ">"})
	}
	// from the end, so that each cut's offsets still hold
	slices.SortFunc(cuts, func(a, b cut) int { return b.start - a.start })
	out := slices.Clone(body)
	for _, c := range cuts {
		out = slices.Concat(out[:c.start], []byte(c.with), out[c.end:])
	}
	return out, nil
}

// jsonSpanAt is the bytes' span [start, end) of the value at path in the JSON document b: each step an object's key
// or `[i]`, an array's item i (dashAt's steps).
func jsonSpanAt(b []byte, path []string) (start, end int, err error) {
	end = len(b)
	for _, step := range path {
		items, err := jsonItems(b[start:end])
		if err != nil {
			return 0, 0, err
		}
		array := bytes.HasPrefix(bytes.TrimLeft(b[start:end], " \t\r\n"), []byte("["))
		n, index := dashIndex(step)
		found := false
		for k, it := range items {
			if array && index && k == n || !array && !index && it.key == step {
				start, end, found = start+it.start, start+it.end, true
				break
			}
		}
		if !found {
			return 0, 0, fmt.Errorf("no %s", step)
		}
	}
	return start, end, nil
}

// fnStreamingView is the admin's netdata-streaming call req of a fn.builtins case on one side, as its exchange
// (fnStreamingViewOf), each problem reported.
func fnStreamingView(t *testing.T, x *fnHTTPSide, label string, req []byte, guard func(Value) error) string {
	t.Helper()
	view, problems := fnStreamingViewOf(x, label, req, guard)
	for _, problem := range problems {
		t.Errorf("%s: %s: %s", x.role, label, problem)
	}
	return view
}

// fnStreamingViewOf asks x's agent req and renders the answer as its exchange (fnStreamingRender), guarded by guard on
// the oracle's side only. It asks once the agent's start window is over (niStartSlack): the ages then read 1 or more
// on both sides, where a call in the start's own second reads 0, which the masks keep, beside the other side's 1.
func fnStreamingViewOf(x *fnHTTPSide, label string, req []byte, guard func(Value) error) (string, []string) {
	launch := x.d.LaunchStartedAt.Unix()
	started := [2]int64{launch, launch + niStartSlack}
	if wait := time.Until(time.Unix(started[1]+1, 0)); wait > 0 {
		time.Sleep(wait)
	}
	from := time.Now().Unix()
	b, err := rawExchange(x.d.Addr, req, fnWait)
	if err != nil {
		return label + ": " + err.Error(), []string{err.Error()}
	}
	_, port, _ := strings.Cut(x.d.Addr, ":")
	if x.role != Oracle {
		guard = nil
	}
	view, problems := fnStreamingRender(b, fnStreamingSide{port, [2]int64{from, time.Now().Unix()}, started}, guard)
	return label + ": " + strconv.Quote(view), problems
}

// fnBuiltinsStreamingInfo: netdata-streaming's words (D241 F3) and payload (H41). C's handler reads none of its
// inputs (function-netdata-streaming.c:21) and the registry finds a method by stripping the words after its name
// (nrpc-registry.c:883-891), so the admin's `netdata-streaming info` answers the table (fnStreamingView), where
// topology:streaming answers its `info` header (function-topology-streaming.c:239-240, :2949); so does a POST with a
// JSON body (`post`: the payload reaches the handler as `payload`, which it does not read; SA-F's probe p1 on C: the
// same table, byte for byte but its times, for a JSON body on v1 and a text body on v3).
func fnBuiltinsStreamingInfo() fnHTTPCase {
	call := fnBuiltinsCall{"admin info", "/api/v1/function?function=netdata-streaming%20info", fnBuiltinsTx(0x311),
		[]string{fnBuiltinsUsers[2].header}}
	post := fnBuiltinsStreamingPost()
	postReq := post.post()
	return fnHTTPCase{
		prepare: fnWriteTokens,
		sc:      fnScenario(plugin.Step{Emit: fnOpenRegister}),
		play: func(t *testing.T, x *fnHTTPSide) []string {
			return []string{fnStreamingView(t, x, call.label, call.request(), fnStreamingStandalone),
				fnStreamingView(t, x, post.label, postReq, fnStreamingStandalone)}
		},
		// the table's length follows its cells
		records: func(l string) string {
			return fnBuiltinsMaskRecord(l, map[string]string{call.tx: "sizes", post.tx: "sizes"})
		},
		wantNot: []string{fnQ(`"accepted_params"`)},
	}
}

// fnBuiltinsStreamingPost is the admin's POST of netdata-streaming with a JSON body (fnBuiltinsStreamingInfo).
func fnBuiltinsStreamingPost() fnBuiltinsCall {
	return fnBuiltinsCall{"admin post", "/api/v1/function?function=netdata-streaming", fnBuiltinsTx(0x312),
		[]string{fnBuiltinsUsers[2].header, "Content-Type: application/json"}}
}

// fnBuiltinsStreamingBody is the POST's payload: words C's handler ignores (function-netdata-streaming.c:21).
const fnBuiltinsStreamingBody = `{"info":true,"after":-600,"x":[1,2]}`

// post is the call as a POST of fnBuiltinsStreamingBody.
func (c fnBuiltinsCall) post() []byte {
	return rawRequest("POST", c.target, append([]string{"X-Transaction-Id: " + c.tx}, c.headers...),
		[]byte(fnBuiltinsStreamingBody))
}

// ---------------------------------------------------------------------------------------------------------------
// The `gone` topology (fn.netdata-streaming)

// fnStreamingChild3 is the `gone` topology's third fixture child, the one that resets its connection.
var fnStreamingChild3 = stream.HostInfo{Hostname: "parity-child3", MachineGUID: "5a1e0000-0000-4000-8000-0000000000f3"}

// fnStreamingGone (TestFnNetdataStreaming/gone): a parent pair (`ram`, one tier, PULSE off, fn.http's bearer tokens)
// and three fixture children (dashChildLinkAs' fixture, the same base on both sides), connected one after the other,
// that leave one after the other, each its own way: the first closes its connection, the second sends a line no
// keyword starts (BOGUS: the receiver's parser refuses it), the third resets its connection (SO_LINGER 0). Each side
// has the three as children before the first leaves (fnStreamHasChild). Each child leaves in a second after the
// previous one's window, so that the three windows are apart: its window runs from that second to the answer that
// showed its detach's stamp (niGoneWindows: C clears the online flag before it stamps the detach's second,
// stream-receiver.c:1467, :1502). Then the admin's table (fnStreamingCompareWith): four rows, the children offline
// with their receivers' stored reasons (fnStreamingGoneGuard), each child's InSince held to its own window, asked again
// while the comparison fails up to dashSettle (the contexts worker takes a child's collected counts down a second
// after it left: SA-F's probe p1).
func fnStreamingGone(t *testing.T) {
	bins := binaries(t)
	p := startPairWith(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, PulseOff: true}, parentIdentity, bins,
		[2]string{}, [2]Role{"fng-p-oracle", "fng-p-candidate"}, fnWriteTokens)
	base := dashBase()
	first, _ := dashChildLinkAs(t, p, base, childHost, qCharts)
	second, _ := dashChildLinkAs(t, p, base, child2Host, qCharts)
	var third [2]net.Conn
	for i, side := range p.Each() {
		nc, err := net.DialTimeout("tcp", side.Daemon.Addr, 10*time.Second)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		t.Cleanup(func() { _ = nc.Close() })
		conn, err := stream.ConnectOn(nc, side.Daemon.StreamKey, fnStreamingChild3, stream.CapsLive)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		streamChartsFixture(t, conn, base, qCharts)
		third[i] = nc
	}
	time.Sleep(2500 * time.Millisecond)
	for _, side := range p.Each() {
		for _, host := range []stream.HostInfo{childHost, child2Host, fnStreamingChild3} {
			if fnStreamHasChild(side.Daemon.Addr, host.MachineGUID, true, 30*time.Second) {
				continue
			}
			if side.Role == Oracle {
				t.Fatalf("oracle: %s is not a child within 30 s", host.Hostname)
			}
			t.Errorf("candidate: %s is not a child within 30 s", host.Hostname)
		}
	}
	ends := []struct {
		host stream.HostInfo
		end  func(i int) error
	}{
		{childHost, func(i int) error { return first[i].Close() }},
		{child2Host, func(i int) error { second[i].Linef("BOGUS"); return second[i].Flush() }},
		{fnStreamingChild3, func(i int) error {
			if tc, ok := third[i].(*net.TCPConn); ok {
				if err := tc.SetLinger(0); err != nil {
					return err
				}
			}
			return third[i].Close()
		}},
	}
	left := [2]map[string][2]int64{{}, {}}
	after := time.Now().Unix()
	for _, e := range ends {
		// a second of its own, later than the children's connections and the previous child's window
		time.Sleep(time.Until(time.Unix(after+1, 0)))
		from := time.Now().Unix()
		for i := range 2 {
			if err := e.end(i); err != nil {
				t.Fatalf("%s: ending its connection: %v", e.host.Hostname, err)
			}
		}
		gone := niGoneWindows(t, p, e.host, from)
		for i := range gone {
			left[i][e.host.Hostname] = gone[i]
		}
		after = max(gone[0][1], gone[1][1])
	}
	// a table asked in the second a child left in reads its age 0 on one side and 1 on the other: ask in a later one
	time.Sleep(time.Until(time.Unix(after+1, 0)))
	fnStreamingCompareWith(t, p, fnStreamingAsk{target: "/api/v1/function?function=netdata-streaming",
		guard: fnStreamingGoneGuard(base), left: left, settle: dashSettle})
	// then each child marked ephemeral by netdatacli (the plan's section 4.3, BACKLOG commit 12's block)
	t.Run("ephemeral", func(t *testing.T) {
		for _, side := range p.Each() {
			for _, host := range []stream.HostInfo{childHost, child2Host, fnStreamingChild3} {
				fnStreamingMarkEphemeral(t, side.Role, side.Daemon, host.MachineGUID)
			}
		}
		fnStreamingCompareWith(t, p, fnStreamingAsk{target: "/api/v1/function?function=netdata-streaming",
			guard: fnStreamingEphemeralGuard(base), left: left, settle: dashSettle})
	})
}

// fnStreamingMarkEphemeral marks the stale node of machine GUID guid ephemeral on d (`netdatacli
// mark-stale-nodes-ephemeral`, daemon/commands.c:586-589). By its GUID C finds the host in memory (:509-514); by its
// hostname it would ask the metadata database's host rows (:519-549, SQL_HOSTNAME_TO_REMOVE), which the metadata
// thread writes a while after the host connected (SA-F's probe p1: `No match` on one side of each run). It is asked
// again each second, up to 30 s, until it says the node is marked (a host whose metadata lock is held answers that it
// is busy, :415-423).
// The oracle's failure ends the test.
func fnStreamingMarkEphemeral(t *testing.T, role Role, d *daemon.Daemon, guid string) {
	t.Helper()
	var last cliResult
	if pollUntil(30*time.Second, func() bool {
		last = runCLI(t, d, "mark-stale-nodes-ephemeral", guid)
		return last.Exit == 0 && strings.Contains(last.Stdout, "has been marked ephemeral")
	}) {
		return
	}
	if role == Oracle {
		t.Fatalf("oracle: %s not marked ephemeral within 30 s: %+v", guid, last)
	}
	t.Errorf("candidate: %s not marked ephemeral within 30 s: %+v", guid, last)
}

// fnStreamingEphemeralGuard is fnStreamingGoneGuard once each child is marked ephemeral: the children's rows say so
// (Ephemerality, function-netdata-streaming.c:141) and an ephemeral host has no severity (`normal`, :107-108), the rest
// as before (rrdhost_option_set(), commands.c:439-441, changes no status).
func fnStreamingEphemeralGuard(base int64) func(Value) error {
	return fnStreamingRows(fnStreamingGoneRows(base, true)...)
}

// fnStreamingGoneGuard is the guard of the oracle's `gone` table, for the fixture's base: localhost initializing,
// then the three children in creation order (dfe over rrdhost_root_index, :75), each not online with a connection
// behind it, so `offline` (rrdhost-status.c:187-192) and of severity critical (function-netdata-streaming.c:107-114),
// with its receiver's stored reason, its first exit reason (stream-receiver.c:1498, :334-338): the closed socket's
// read (:1036), the parser's refusal (:868), the reset socket's poll event (:1086, HUP); without a receiver its
// socket and capabilities empty (rrdhost-status.c:209-220); its contexts worker has taken its collected counts down
// (rrdcontext-worker.c:1125-1127) and stored its retention, the fixture's minute (base to base+60:
// rrdcontext-worker.c:1126, :82-101; streamChartsFixture), which the table prints, not the clock (rrdhost.h:617-618).
func fnStreamingGoneGuard(base int64) func(Value) error {
	return fnStreamingRows(fnStreamingGoneRows(base, false)...)
}

// fnStreamingGoneRows are fnStreamingGoneGuard's rows, the children's marked ephemeral where ephemeral says
// (fnStreamingEphemeralGuard).
func fnStreamingGoneRows(base int64, ephemeral bool) []fnStreamingRow {
	child := func(host stream.HostInfo, reason string) fnStreamingRow {
		row := fnStreamingRow{"Node": strconv.Quote(host.Hostname), "rowOptions": `{"severity":"critical"}`,
			"InStatus": `"offline"`, "InReason": strconv.Quote(reason), "InLocalIP": `""`, "InLocalPort": "0",
			"InRemoteIP": `""`, "InRemotePort": "0", "InCapabilities": "[]", "CollectedMetrics": "0",
			"dbFrom": strconv.FormatInt(base*1000, 10), "dbTo": strconv.FormatInt((base+60)*1000, 10)}
		if ephemeral {
			row["rowOptions"], row["Ephemerality"] = `{"severity":"normal"}`, `"ephemeral"`
		}
		return row
	}
	local := fnStreamingRow{"Node": strconv.Quote(parentIdentity.Hostname), "InReason": `"LOCALHOST"`,
		"InStatus": `"initializing"`}
	if ephemeral {
		// the command marks the stale nodes alone (commands.c:426-436: an online host is left as it is)
		local["Ephemerality"] = `"permanent"`
	}
	return []fnStreamingRow{
		local,
		child(childHost, "DISCONNECTED SOCKET CLOSED BY REMOTE END"),
		child(child2Host, "DISCONNECTED PARSE ERROR"),
		child(fnStreamingChild3, "DISCONNECTED SOCKET CLOSED BY REMOTE END"),
	}
}
