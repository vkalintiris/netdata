// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"maps"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"
)

// The `netdata-streaming` built-in (milestone 10 commit 12): C's table of every host's status
// (web/api/functions/function-netdata-streaming.c:21-1014), which reads the host status of every host
// (rrdhost_status, :77). `fn.builtins` compares the admin's table of a standalone agent (fnStreamingView).

// TestFnNetdataStreaming (check `fn.netdata-streaming`, milestone 10 commit 12): the admin's
// `/api/v1/function?function=netdata-streaming` on each agent of four topologies: `calls`, `fn.stream`'s (two parents,
// a real C child each, the child's vnode released after it) once its cases played; `gone`, three fixture children
// that left (fnStreamingGone); `never` and `out`, TestNodeInstancesStream's agents streaming to a scripted parent, at
// each of their stages (fnStreamingOut).
func TestFnNetdataStreaming(t *testing.T) {
	t.Run("calls", func(t *testing.T) {
		topo := fnStreamCalls()
		topo.compare = fnStreamingCompare
		runFnStream(t, topo)
	})
	t.Run("gone", fnStreamingGone)
	// the Out cells, on TestNodeInstancesStream's pairs (fnstreaming_out_test.go)
	t.Run("never", func(t *testing.T) { runNIStream(t, "never", fnStreamingOut) })
	t.Run("out", func(t *testing.T) { runNIStream(t, "out", fnStreamingOut) })
}

// fnStreamingTx is the admin's call's transaction (fnStreamTx's scheme, beyond the `calls` cases' numbers).
var fnStreamingTx = fnStreamTx(0x100)

// fnStreamingExpiresRe is fnBuiltinsExpires' rendering of the expiry, which the JSON parser needs as a string.
var fnStreamingExpiresRe = regexp.MustCompile(`"expires":(NOW[+-][0-9]+)`)

// fnStreamingDoc is a response's body as JSON, its expiry rendered as its distance from the head's Date (the
// handler writes now + 10, :1010; fnBuiltinsExpires).
func fnStreamingDoc(resp []byte) ([]byte, Value, error) {
	body := fnBuiltinsExpires(httpBody(resp), fnHTTPDate(resp))
	body = fnStreamingExpiresRe.ReplaceAll(body, []byte(`"expires":"$1"`))
	v, err := ParseJSON(body)
	return body, v, err
}

// fnStreamingVolatile are the table's columns whose cells (`data.[].[<index>]`) and maxima (`columns.<name>.max`)
// differed C against C, by name: the retention's ends and length (:155-167), the status changes' times and ages
// (:185-197, :235-247) and each inbound connection's ports (:214-219). Only their numbers other than 0 are
// masked (fnStreamingMasks): localhost's retention start, length and ports read 0 or null in every C run. The
// outbound ports, traffic and attempts stay compared: the parents stream nowhere, so C writes them 0 or null. What a
// mask hides is held, on each side, to what C fixes between the cells (fnStreamingFacts).
var fnStreamingVolatile = []string{
	"dbFrom", "dbTo", "dbDuration",
	"InSince", "InAge", "OutSince", "OutAge",
	"InLocalPort", "InRemotePort",
}

// fnStreamingColumns are a table's column indexes by name (`columns.<name>.index`).
func fnStreamingColumns(v Value) map[string]int {
	out := map[string]int{}
	cols, err := dashMember(v, "columns")
	if err != nil {
		return out
	}
	for _, c := range cols.Members {
		if i, err := dashMember(c.Value, "index"); err == nil {
			if n, err := strconv.Atoi(i.Text); err == nil {
				out[c.Key] = n
			}
		}
	}
	return out
}

// fnStreamingMasks are the masks of v's cells of the columns names (`data.[r].[i]`, at each column's own index) and
// of their maxima (`columns.<name>.max`) that hold a number other than 0: a 0 or a null stays compared, and so does
// the retention of a host that is not online (fnStreamingRetention: what it stored, not the clock).
func fnStreamingMasks(v Value, names []string) []Mask {
	cols := fnStreamingColumns(v)
	data, _ := dashMember(v, "data")
	var masks []Mask
	for _, n := range names {
		i, ok := cols[n]
		if !ok {
			continue
		}
		for r, row := range data.Items {
			if slices.Contains(fnStreamingRetention, n) && fnStreamingOffline(v, r) {
				continue
			}
			if i < len(row.Items) && fnStreamingNonZero(row.Items[i]) {
				masks = append(masks, Mask{fmt.Sprintf("data.[%d].[%d]", r, i), n})
			}
		}
		if top, err := dashAt(v, "columns", n, "max"); err == nil && fnStreamingNonZero(top) {
			masks = append(masks, Mask{"columns." + n + ".max", n})
		}
	}
	return masks
}

// fnStreamingRetention are the columns of a host's retention (:155-167): for a host that is online its end is the
// handler's clock and its start the host's first stored second (rrdhost_retention(), rrdhost.h:607-619, `now` when
// online); for one that is not, the two its contexts worker stored when the host left (rrdcontext-worker.c:1126,
// :82-101), which a fixture child's data fixes.
var fnStreamingRetention = []string{"dbFrom", "dbTo", "dbDuration"}

// fnStreamingOffline tells whether row r's host is not online: its InStatus is `offline` or `archived`, the two
// statuses of a host that is not local and has no collector (rrdhost-status.c:187-192, :375).
func fnStreamingOffline(v Value, r int) bool {
	c, err := fnStreamingCell(v, r, "InStatus")
	return err == nil && (c.Text == "offline" || c.Text == "archived")
}

// fnStreamingNonZero tells whether v is a number other than 0.
func fnStreamingNonZero(v Value) bool {
	x, err := strconv.ParseFloat(v.Text, 64)
	return v.Kind == KindNumber && err == nil && x != 0
}

// fnStreamingCell is row r's cell of column name.
func fnStreamingCell(v Value, r int, name string) (Value, error) {
	data, err := dashMember(v, "data")
	if err != nil {
		return Value{}, err
	}
	i, ok := fnStreamingColumns(v)[name]
	if !ok || r >= len(data.Items) || i >= len(data.Items[r].Items) {
		return Value{}, fmt.Errorf("no cell %s of row %d", name, r)
	}
	return data.Items[r].Items[i], nil
}

// fnStreamingWhole is a cell or a maximum as a whole number: null tells a JSON null apart (C's cell of a host without
// the value).
func fnStreamingWhole(v Value) (n int64, null bool, err error) {
	if v.Kind == KindNull {
		return 0, true, nil
	}
	if v.Kind != KindNumber {
		return 0, false, fmt.Errorf("%s is not a number", v)
	}
	n, err = strconv.ParseInt(v.Text, 10, 64)
	return n, false, err
}

// fnStreamingFacts holds one side's table to what C fixes between the cells the masks hide (fnStreamingVolatile),
// given the port the side's parent listens on and the seconds the request was in flight
// (function-netdata-streaming.c; C's table in H35's probe P1 held every one on both sides):
//   - the handler reads its clock once (:22): each row's `InSince` and `OutSince` are milliseconds of a whole second
//     and its `InAge` and `OutAge` the seconds from there to that clock (:185-192, :235-242), one second of the flight
//     for every row; a host without the time has both null (:194-197, :244-247);
//   - `dbFrom` and `dbTo` are milliseconds of whole seconds (:155, :158), `dbTo` that clock for a host that is online
//     (rrdhost.h:617-618; not for one that is offline or archived, fnStreamingOffline: its stored end), and
//     `dbDuration` the seconds between them, null where there is no start (:161-167);
//   - a host with a receiver (its reason is the handshake's, `CONNECTED`) names the receiver's socket: `InLocalPort`
//     the port the parent listens on and `InRemotePort` another one, the child's (:214-219, socket-peers.c:22-50); a
//     host without one has both 0;
//   - each of these columns' `max` is the largest of its cells, 0 without any (:156-242, :568-738).
func fnStreamingFacts(v Value, port string, flight [2]int64) error {
	data, err := dashMember(v, "data")
	if err != nil {
		return err
	}
	listen, err := strconv.ParseInt(port, 10, 64)
	if err != nil {
		return fmt.Errorf("harness: the parent's port %q: %v", port, err)
	}
	clock, clocked := int64(0), false
	top := map[string]int64{}
	for r := range data.Items {
		cell := map[string]int64{}
		null := map[string]bool{}
		for _, name := range fnStreamingVolatile {
			c, err := fnStreamingCell(v, r, name)
			if err != nil {
				return err
			}
			if cell[name], null[name], err = fnStreamingWhole(c); err != nil || cell[name] < 0 {
				return fmt.Errorf("row %d: %s is %s, want a whole number or null", r, name, c)
			}
			if !null[name] {
				top[name] = max(top[name], cell[name])
			}
		}
		for _, side := range []string{"In", "Out"} {
			since, age := side+"Since", side+"Age"
			if null[since] != null[age] {
				return fmt.Errorf("row %d: %s and %s are not both null", r, since, age)
			}
			if null[since] {
				continue
			}
			now := cell[since]/1000 + cell[age]
			if cell[since]%1000 != 0 || now < flight[0] || now > flight[1] || (clocked && now != clock) {
				return fmt.Errorf("row %d: %s %d and %s %d: not the milliseconds of a second and the seconds since, "+
					"up to one second of the request's flight [%d, %d] for every row", r, since, cell[since], age,
					cell[age], flight[0], flight[1])
			}
			clock, clocked = now, true
		}
		if null["dbFrom"] || null["dbTo"] || cell["dbFrom"]%1000 != 0 || cell["dbTo"]%1000 != 0 ||
			(clocked && !fnStreamingOffline(v, r) && cell["dbTo"] != clock*1000) {
			return fmt.Errorf("row %d: dbFrom %d and dbTo %d: not milliseconds of whole seconds, the last one the "+
				"table's clock (%d) where the host is online", r, cell["dbFrom"], cell["dbTo"], clock)
		}
		if spans := cell["dbFrom"] > 0 && cell["dbTo"] > cell["dbFrom"]; null["dbDuration"] == spans ||
			(spans && cell["dbDuration"] != (cell["dbTo"]-cell["dbFrom"])/1000) {
			return fmt.Errorf("row %d: dbDuration is not the seconds from dbFrom %d to dbTo %d", r, cell["dbFrom"],
				cell["dbTo"])
		}
		reason, err := fnStreamingCell(v, r, "InReason")
		if err != nil {
			return err
		}
		local, remote := cell["InLocalPort"], cell["InRemotePort"]
		if null["InLocalPort"] || null["InRemotePort"] {
			return fmt.Errorf("row %d: a port is null", r)
		}
		if received := reason.Text == "CONNECTED"; (received && (local != listen || remote <= 0 || remote > 65535 ||
			remote == listen)) || (!received && (local != 0 || remote != 0)) {
			return fmt.Errorf("row %d (%s): InLocalPort %d and InRemotePort %d, want the parent's port %d and the "+
				"child's for a host with a receiver, 0 and 0 without one", r, reason.Text, local, remote, listen)
		}
	}
	if !clocked {
		return fmt.Errorf("no row has a time")
	}
	for _, name := range fnStreamingVolatile {
		m, err := dashAt(v, "columns", name, "max")
		if err != nil {
			return err
		}
		if x, perr := strconv.ParseFloat(m.Text, 64); m.Kind != KindNumber || perr != nil || x != float64(top[name]) {
			return fmt.Errorf("columns.%s.max is %s, the largest cell is %d", name, m, top[name])
		}
	}
	return nil
}

// fnStreamingGuard checks the oracle's table of the `calls` topology: three hosts in creation order (dfe over
// rrdhost_root_index, :75): the parent's localhost, still initializing with no metric of its own
// (rrdhost-status.c:124-130, :172-173), then the child and its vnode, each online on a receiver of its own, so each
// one's inbound reason is its handshake's (rrdhost-status.c:213, :225-230: the receiver before the virtual flag;
// :199-204; stream-handshake.c:10-12).
var fnStreamingGuard = fnStreamingRows(
	fnStreamingRow{"Node": strconv.Quote(parentIdentity.Hostname), "InReason": `"LOCALHOST"`, "InStatus": `"initializing"`},
	fnStreamingRow{"Node": strconv.Quote(rchildHostname), "InReason": `"CONNECTED"`, "InStatus": `"online"`},
	fnStreamingRow{"Node": strconv.Quote(rvName), "InReason": `"CONNECTED"`, "InStatus": `"online"`},
)

// fnStreamingRow is what the oracle's table must show of one host: cells by column name, each as Value.String()
// writes it (a string quoted, rowOptions as its object).
type fnStreamingRow map[string]string

// fnStreamingRows is the guard of a table of exactly these hosts, in this order (the host index's creation order,
// dfe over rrdhost_root_index, :75), each holding the cells its row names.
func fnStreamingRows(rows ...fnStreamingRow) func(Value) error {
	return func(v Value) error {
		if err := dashIs(`"table"`, "type")(v); err != nil {
			return err
		}
		data, err := dashMember(v, "data")
		if err != nil {
			return err
		}
		if len(data.Items) != len(rows) {
			return fmt.Errorf("%d rows, want %d", len(data.Items), len(rows))
		}
		for r, want := range rows {
			for _, col := range slices.Sorted(maps.Keys(want)) {
				c, err := fnStreamingCell(v, r, col)
				if err != nil {
					return err
				}
				if c.String() != want[col] {
					return fmt.Errorf("row %d: %s is %s, want %s", r, col, c, want[col])
				}
			}
		}
		return nil
	}
}

// fnStreamingAsk is one comparison of the admin's table (fnStreamingCompareWith): the request's target, the
// oracle's guard, by side the hosts that left and the seconds each left in (fnStreamingSince), how long the sides may
// be asked again while the comparison fails (0: once), and, where the topology needs them, more columns masked as
// fnStreamingVolatile's (volatile) and the facts that hold side i's table besides (facts).
type fnStreamingAsk struct {
	target   string
	guard    func(Value) error
	left     [2]map[string][2]int64
	settle   time.Duration
	volatile []string
	facts    func(i int, v Value, flight [2]int64) error
}

// fnStreamingCompare is the `calls` topology's comparison (fnStreamingCompareWith): the table of v1, fnStreamingGuard,
// asked once.
func fnStreamingCompare(t *testing.T, p *Pair, _ [2]*fnStreamSide) {
	fnStreamingCompareWith(t, p, fnStreamingAsk{target: "/api/v1/function?function=netdata-streaming",
		guard: fnStreamingGuard})
}

// fnStreamingCompareWith asks each parent for the admin's netdata-streaming table and compares the answers: the
// oracle's status and guard first, the heads (fnHTTPMask; each side's length against its own body), each side's
// table against what C fixes between its cells (fnStreamingFacts) and the seconds its times come from
// (fnStreamingSince: the agent's start window, niStartSlack, and ask.left), then the bodies as ordered JSON with the
// volatile columns' numbers other than 0 masked by name (fnStreamingMasks), their layouts and their strings'
// escapes. While that fails both are asked again, up to ask.settle; the last round's failures are reported, the
// oracle's ending the test. The request is the Functions checks' own (fnHTTPGet, a transaction of this check's), so
// the sequence is written here and not compareV2's.
func fnStreamingCompareWith(t *testing.T, p *Pair, ask fnStreamingAsk) {
	t.Run("netdata-streaming", func(t *testing.T) {
		deadline := time.Now().Add(ask.settle)
		r := fnStreamingRound(p, ask)
		for (len(r.fatal) > 0 || len(r.problems) > 0) && time.Now().Before(deadline) {
			time.Sleep(time.Second)
			r = fnStreamingRound(p, ask)
		}
		if len(r.fatal) > 0 {
			t.Fatalf("%s", r.fatal)
		}
		for _, problem := range r.problems {
			t.Errorf("%s", problem)
		}
		if t.Failed() {
			t.Logf("oracle:\n%s", r.body[0])
		}
	})
}

// fnStreamingRounds is one round of fnStreamingCompareWith: the oracle's failure that ends the test (empty when
// none), the other failures, the bodies.
type fnStreamingRounds struct {
	fatal    string
	problems []string
	body     [2][]byte
}

// fnStreamingRound asks both sides once and judges the answers (fnStreamingCompareWith).
func fnStreamingRound(p *Pair, ask fnStreamingAsk) (r fnStreamingRounds) {
	req := fnHTTPGet(ask.target, fnStreamingTx, fnBuiltinsUsers[2].header)
	var raw [2][]byte
	var flight [2][2]int64
	for i, side := range p.Each() {
		from := time.Now().Unix()
		b, err := rawExchange(side.Daemon.Addr, req, fnWait)
		if err != nil {
			r.fatal = fmt.Sprintf("%s: %v", side.Role, err)
			return r
		}
		raw[i], flight[i] = b, [2]int64{from, time.Now().Unix()}
	}
	r.body[0] = raw[0]
	if s := fnStatusLine(raw[0]); s != "HTTP/1.1 200 OK" {
		r.fatal = fmt.Sprintf("oracle: answered %q", truncateBytes(raw[0]))
		return r
	}
	var doc [2]Value
	var err error
	if r.body[0], doc[0], err = fnStreamingDoc(raw[0]); err != nil {
		r.fatal = fmt.Sprintf("oracle: %v: %s", err, truncateBytes(raw[0]))
		return r
	}
	if err := ask.guard(doc[0]); err != nil {
		r.fatal = fmt.Sprintf("oracle: %v: %s", err, truncateBytes(r.body[0]))
		return r
	}
	var head [2]string
	for i, side := range p.Each() {
		h, _, _ := bytes.Cut(raw[i], []byte("\r\n\r\n"))
		if err := rawLength(raw[i]); err != nil {
			r.problems = append(r.problems, fmt.Sprintf("%s: %v", side.Role, err))
		}
		head[i] = contentLengthRe.ReplaceAllString(fnHTTPMask(h), "Content-Length: <masked>")
	}
	if head[0] != head[1] {
		r.problems = append(r.problems, fmt.Sprintf("heads differ\noracle:    %q\ncandidate: %q", head[0], head[1]))
	}
	if r.body[1], doc[1], err = fnStreamingDoc(raw[1]); err != nil {
		r.problems = append(r.problems, fmt.Sprintf("candidate: %v: %s", err, truncateBytes(raw[1])))
		return r
	}
	for i, side := range p.Each() {
		_, port, _ := strings.Cut(side.Daemon.Addr, ":")
		launch := side.Daemon.LaunchStartedAt.Unix()
		err := fnStreamingFacts(doc[i], port, flight[i])
		if err == nil {
			err = fnStreamingSince(doc[i], [2]int64{launch, launch + niStartSlack}, ask.left[i])
		}
		if err == nil && ask.facts != nil {
			err = ask.facts(i, doc[i], flight[i])
		}
		if err == nil {
			continue
		}
		if side.Role == Oracle {
			r.fatal = fmt.Sprintf("oracle: %v: %s", err, truncateBytes(r.body[0]))
			return r
		}
		r.problems = append(r.problems, fmt.Sprintf("%s: %v", side.Role, err))
	}
	volatile := slices.Concat(fnStreamingVolatile, ask.volatile)
	o := ApplyMasks(doc[0], fnStreamingMasks(doc[0], volatile))
	c := ApplyMasks(doc[1], fnStreamingMasks(doc[1], volatile))
	for _, d := range Compare(o, c) {
		r.problems = append(r.problems, d.String())
	}
	if l := v2Layouts(r.body, false); l != "" {
		r.problems = append(r.problems, l)
	}
	if e := v2Escapes(r.body, doc, v2Family{}); e != "" {
		r.problems = append(r.problems, e)
	}
	return r
}
