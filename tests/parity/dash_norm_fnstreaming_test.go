// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"
)

// testDashNormFnStreaming pins the judges of the netdata-streaming rows (fnstreaming_test.go,
// fnstreaming_more_test.go, fnstreaming_out_test.go) on the answers C gave in SA-F's probes p2 and p1b
// (dash_norm_fnstreaming_data_test.go): the standalone table's view (fnStreamingRender, with jsonSpanAt and
// fnStreamingMaskText under it), the times' windows (fnStreamingSince), the guards (fnStreamingRows), the masks and
// facts of a host that is not online (fnStreamingMasks, fnStreamingFacts), the Out cells of the stream pairs
// (fnStreamingOutGuard, fnStreamingOutFacts, fnStreamingOutVolatile) and the comparison's wiring (fnStreamingRound,
// asked of two stub agents). Each first holds C's own pair, then refuses named wrong candidates, each one thing wrong
// in the candidate's recorded answer.
func testDashNormFnStreaming(t *testing.T) {
	t.Run("render", testFnsRender)
	t.Run("since", testFnsSince)
	t.Run("rows", testFnsRows)
	t.Run("masks", testFnsMasks)
	t.Run("round", testFnsRound)
	t.Run("out", testFnsOut)
	t.Run("text", testFnsText)
}

// fnsCell is one cell of a recorded table to write anew: row r's cell of col (r -1: the column's `max`) as text.
type fnsCell struct {
	r    int
	col  string
	text string
}

// fnsWith is resp with its body's cells written as cells say (jsonSpanAt) and its Content-Length the new body's.
func fnsWith(t *testing.T, resp string, cells ...fnsCell) string {
	t.Helper()
	head, body, _ := strings.Cut(resp, "\r\n\r\n")
	v, err := ParseJSON([]byte(body))
	if err != nil {
		t.Fatalf("harness: %v", err)
	}
	cols := fnStreamingColumns(v)
	for _, c := range cells {
		path := []string{"columns", c.col, "max"}
		if c.r >= 0 {
			path = []string{"data", "[" + strconv.Itoa(c.r) + "]", "[" + strconv.Itoa(cols[c.col]) + "]"}
		}
		start, end, err := jsonSpanAt([]byte(body), path)
		if err != nil {
			t.Fatalf("harness: %v: %v", path, err)
		}
		body = body[:start] + c.text + body[end:]
	}
	return fnsLength(head, body)
}

// fnsReplace is resp with the first n of old in its body replaced by new (n -1: every one), its Content-Length the
// new body's; old must be there.
func fnsReplace(t *testing.T, resp, old, new string, n int) string {
	t.Helper()
	head, body, _ := strings.Cut(resp, "\r\n\r\n")
	if !strings.Contains(body, old) {
		t.Fatalf("harness: the recorded body has no %q", old)
	}
	return fnsLength(head, strings.Replace(body, old, new, n))
}

// fnsLength is head and body as one response, with the Content-Length of body.
func fnsLength(head, body string) string {
	return contentLengthRe.ReplaceAllString(head, "Content-Length: "+strconv.Itoa(len(body))) + "\r\n\r\n" + body
}

// fnsCellOf is row r's cell of col of a recorded answer, as text.
func fnsCellOf(t *testing.T, resp string, r int, col string) string {
	t.Helper()
	_, v, err := fnStreamingDoc([]byte(resp))
	if err != nil {
		t.Fatalf("harness: %v", err)
	}
	c, err := fnStreamingCell(v, r, col)
	if err != nil {
		t.Fatalf("harness: %v", err)
	}
	return c.String()
}

// fnsInt is a whole number's text as a number.
func fnsInt(t *testing.T, s string) int64 {
	t.Helper()
	n, err := strconv.ParseInt(s, 10, 64)
	if err != nil {
		t.Fatalf("harness: %v", err)
	}
	return n
}

// fnsSideOf is what a recorded answer's side holds it to (fnStreamingRender).
func fnsSideOf(r fnsRecorded) fnStreamingSide {
	return fnStreamingSide{r.port, r.flight, [2]int64{r.launch, r.launch + niStartSlack}}
}

// testFnsRender: the standalone table as fn.builtins' exchanges compare it (fnStreamingRender).
func testFnsRender(t *testing.T) {
	render := func(r fnsRecorded, resp string, guard func(Value) error) (string, string) {
		view, problems := fnStreamingRender([]byte(resp), fnsSideOf(r), guard)
		return view, strings.Join(problems, "; ")
	}
	o, c := fnsStandalone[0], fnsStandalone[1]
	ov, op := render(o, o.resp, fnStreamingStandalone)
	cv, cp := render(c, c.resp, nil)
	if op != "" || cp != "" {
		t.Errorf("C's standalone tables: problems %q, %q", op, cp)
	}
	if ov != cv {
		t.Errorf("C's two standalone tables render differently: %s", firstDifference([]byte(ov), []byte(cv)))
	}
	// what the view hides: localhost's retention end, its two times and two ages, and their five maxima (its
	// retention start, length and ports read 0 or null, compared)
	for _, name := range []string{"dbTo", "InSince", "InAge", "OutSince", "OutAge"} {
		if n := strings.Count(ov, "<masked:"+name+">"); n != 2 {
			t.Errorf("the view masks %s %d times, want 2 (the cell and the maximum)", name, n)
		}
	}
	if n := strings.Count(ov, "<masked:"); n != 10 {
		t.Errorf("the view masks %d values, want 10", n)
	}
	// `info` is no argument of this Function: C's table to `netdata-streaming info` is the plain call's, but for the
	// transaction (fnBuiltinsStreamingInfo)
	iv, ip := render(fnsInfo[0], fnsInfo[0].resp, fnStreamingStandalone)
	iv = strings.Replace(iv, fnBuiltinsTx(0x311), "TX", 1)
	if pv := strings.Replace(ov, fnBuiltinsTx(3), "TX", 1); ip != "" || iv != pv {
		t.Errorf("C's `info` table: problems %q; renders as the plain one: %t: %s", ip, iv == pv,
			firstDifference([]byte(pv), []byte(iv)))
	}

	clock := fnsInt(t, fnsCellOf(t, c.resp, 0, "dbTo")) / 1000
	since := fnsInt(t, fnsCellOf(t, c.resp, 0, "InSince"))
	age := fnsCellOf(t, c.resp, 0, "InAge")
	date := c.resp[strings.Index(c.resp, "Date: ")+6:]
	date = date[:strings.Index(date, "\r\n")]
	for name, w := range map[string]struct {
		resp    string
		problem string // what a problem says; none for a view that differs from C's
	}{
		// the Rust build's answer until commit 12 (D176.3): no table
		"D176.3's 501": {fnsLength("HTTP/1.1 501 Not Implemented\r\nDate: "+date+"\r\nContent-Type: application/json\r\n"+
			"Content-Length: 0", `{"status":501,"errorMessage":"This feature is not implemented yet on this agent."}`), ""},
		// topology:streaming's `info` branch copied over: its header, no table (function-topology-streaming.c:260-273)
		"topology:streaming's info header": {fnsLength(c.resp[:strings.Index(c.resp, "\r\n\r\n")], "{\n"+
			`    "status":200,`+"\n"+`    "type":"topology",`+"\n"+`    "update_every":10,`+"\n"+
			`    "has_history":false,`+"\n"+`    "accepted_params":["info"],`+"\n"+`    "required_params":[],`+"\n"+
			fmt.Sprintf(`    "expires":%d`, clock+10)+"\n}\n"), "no data"},
		// localhost's two times the call's second, its ages 0: no longer the agent's start
		"localhost's start the call's second": {fnsWith(t, c.resp, fnsCell{0, "InSince", strconv.FormatInt(clock*1000, 10)},
			fnsCell{0, "InAge", "0"}, fnsCell{-1, "InSince", strconv.FormatInt(clock*1000, 10)}, fnsCell{-1, "InAge", "0"}),
			"InSince is " + strconv.FormatInt(clock*1000, 10) + ", want the milliseconds of a second of the agent's start"},
		// the outbound time from another reading of the start than the inbound one
		"OutSince a second after InSince": {fnsWith(t, c.resp, fnsCell{0, "OutSince", strconv.FormatInt(since+1000, 10)},
			fnsCell{0, "OutAge", strconv.FormatInt(fnsInt(t, age)-1, 10)},
			fnsCell{-1, "OutSince", strconv.FormatInt(since+1000, 10)},
			fnsCell{-1, "OutAge", strconv.FormatInt(fnsInt(t, age)-1, 10)}), "another cell read the start as"},
		// the facts on the standalone row: an age from another clock
		"an age a second off": {fnsWith(t, c.resp, fnsCell{0, "InAge", strconv.FormatInt(fnsInt(t, age)+1, 10)},
			fnsCell{-1, "InAge", strconv.FormatInt(fnsInt(t, age)+1, 10)}), "row 0: InSince"},
		"the api-calls' expiry, now + 1": {fnsReplace(t, c.resp, fmt.Sprintf(`"expires":%d`, fnsInt(t,
			fnsMember(t, c.resp, "expires"))), fmt.Sprintf(`"expires":%d`, clock+1), 1), ""},
		"another agent name":   {fnsReplace(t, c.resp, `"permanent","netdata",`, `"permanent","netdata-rs",`, 1), ""},
		"a slash escaped":      {fnsReplace(t, c.resp, `"/etc/os-release"`, `"\/etc\/os-release"`, 1), ""},
		"the severity warning": {fnsReplace(t, c.resp, `"severity":"normal"`, `"severity":"warning"`, 1), ""},
		"InLocalPort the listening port": {fnsWith(t, c.resp, fnsCell{0, "InLocalPort", c.port},
			fnsCell{-1, "InLocalPort", c.port}), "row 0 (LOCALHOST): InLocalPort"},
	} {
		got, problems := render(c, w.resp, nil)
		switch {
		case w.problem == "" && got == ov:
			t.Errorf("%s: renders as C's", name)
		case w.problem != "" && !strings.Contains(problems, w.problem):
			t.Errorf("%s: problems %q, want %q", name, problems, w.problem)
		}
	}
	// the length follows the cells: a table started ten seconds earlier (its ages one digit longer) renders as C's
	// (its problems are the start window's, not the view's)
	longer := fnsWith(t, c.resp, fnsCell{0, "InSince", strconv.FormatInt(since-10000, 10)},
		fnsCell{0, "InAge", strconv.FormatInt(fnsInt(t, age)+10, 10)}, fnsCell{0, "OutSince",
			strconv.FormatInt(since-10000, 10)}, fnsCell{0, "OutAge", strconv.FormatInt(fnsInt(t, age)+10, 10)},
		fnsCell{-1, "InSince", strconv.FormatInt(since-10000, 10)}, fnsCell{-1, "InAge", strconv.FormatInt(fnsInt(t, age)+10,
			10)}, fnsCell{-1, "OutSince", strconv.FormatInt(since-10000, 10)}, fnsCell{-1, "OutAge",
			strconv.FormatInt(fnsInt(t, age)+10, 10)})
	if got, _ := render(c, longer, nil); len(longer) == len(c.resp) || got != ov {
		t.Errorf("a longer table (%d bytes, C's %d) does not render as C's: %s", len(longer), len(c.resp),
			firstDifference([]byte(ov), []byte(got)))
	}
	// the length a byte short: the head's length is checked against the body's own (rawLength), then masked
	short := strings.Replace(c.resp, "Content-Length: ", "Content-Length: 9", 1)
	if _, problems := render(c, short, nil); !strings.Contains(problems, "Content-Length is 9") {
		t.Errorf("a length that is not the body's: problems %q", problems)
	}
	// the oracle's guard: a table of another state, or no 200
	for name, w := range map[string]struct{ resp, problem string }{
		"the oracle's severity warning": {fnsReplace(t, o.resp, `"severity":"normal"`, `"severity":"warning"`, 1),
			`guard: row 0: rowOptions is {"severity":"warning"}`},
		"the oracle's 500":   {strings.Replace(o.resp, "HTTP/1.1 200 OK", "HTTP/1.1 500 Internal Server Error", 1), "answered"},
		"the oracle's child": {fnsReplace(t, o.resp, `"LOCALHOST"`, `"CONNECTED"`, 1), "guard: row 0: InReason"},
	} {
		if _, problems := render(o, w.resp, fnStreamingStandalone); !strings.Contains(problems, w.problem) {
			t.Errorf("%s: problems %q, want %q", name, problems, w.problem)
		}
	}
	// fnStreamingViewOf asks a stub agent writing C's recorded table at the second it answers in (fnsClock), launched in
	// its recorded second: the oracle's side is guarded, the candidate's is not
	warned := fnsClockOf(t, fnsReplace(t, c.resp, `"severity":"normal"`, `"severity":"warning"`, 1))
	for role, want := range map[Role]string{Oracle: `guard: row 0: rowOptions is {"severity":"warning"}`, Candidate: ""} {
		d := dashNormStub(t, stubAnswer{body: warned.at}.raw)
		d.LaunchStartedAt = time.Unix(c.launch, 0)
		_, problems := fnStreamingViewOf(&fnHTTPSide{role: role, d: d}, "v1", fnHTTPGet(
			"/api/v1/function?function=netdata-streaming", fnBuiltinsTx(3), fnBuiltinsUsers[2].header), fnStreamingStandalone)
		if got := strings.Join(problems, "; "); (want == "") != (got == "") || !strings.Contains(got, want) {
			t.Errorf("fnStreamingViewOf as the %s: problems %q, want %q", role, got, want)
		}
	}
	// no guard on the candidate: its table is judged by comparison and facts only
	if _, problems := render(c, fnsReplace(t, c.resp, `"severity":"normal"`, `"severity":"warning"`, 1), nil); len(problems) != 0 {
		t.Errorf("the candidate's severity: problems %q, want none (the comparison's)", problems)
	}
}

// fnsMember is a recorded answer's top-level member, as text.
func fnsMember(t *testing.T, resp, key string) string {
	t.Helper()
	_, body, _ := strings.Cut(resp, "\r\n\r\n")
	v, err := ParseJSON([]byte(body))
	if err != nil {
		t.Fatalf("harness: %v", err)
	}
	m, err := dashMember(v, key)
	if err != nil {
		t.Fatalf("harness: %v", err)
	}
	return m.String()
}

// fnsDoc is a recorded answer's body as JSON (fnStreamingDoc).
func fnsDoc(t *testing.T, resp string) Value {
	t.Helper()
	_, v, err := fnStreamingDoc([]byte(resp))
	if err != nil {
		t.Fatalf("harness: %v", err)
	}
	return v
}

// testFnsSince: the seconds the times come from (fnStreamingSince).
func testFnsSince(t *testing.T) {
	problem := func(err error) string {
		if err != nil {
			return err.Error()
		}
		return ""
	}
	window := func(r fnsRecorded) [2]int64 { return [2]int64{r.launch, r.launch + niStartSlack} }
	for i, r := range fnsStandalone {
		if err := fnStreamingSince(fnsDoc(t, r.resp), window(r), nil); err != nil {
			t.Errorf("C's standalone table %d: %v", i, err)
		}
	}
	for i, r := range fnsGone {
		if err := fnStreamingSince(fnsDoc(t, r.resp), window(r), r.left); err != nil {
			t.Errorf("C's gone table %d: %v", i, err)
		}
	}
	c := fnsGone[1]
	since := fnsInt(t, fnsCellOf(t, c.resp, 0, "InSince"))
	child := fnsInt(t, fnsCellOf(t, c.resp, 1, "InSince"))
	w := c.left[childHost.Hostname]
	for name, x := range map[string]struct {
		resp    string
		started [2]int64
		left    map[string][2]int64
		want    string
	}{
		"a start before the launch": {c.resp, [2]int64{since/1000 + 1, since/1000 + 6}, c.left,
			"row 0 (parity-parent): InSince is"},
		"a start after the window": {c.resp, [2]int64{since/1000 - 6, since/1000 - 1}, c.left,
			"row 0 (parity-parent): InSince is"},
		"a child's OutSince not the start": {fnsWith(t, c.resp, fnsCell{1, "OutSince", strconv.FormatInt(since+1000, 10)}),
			window(c), c.left, "row 1 (parity-child): OutSince is the second"},
		"a start that is no whole second": {fnsWith(t, c.resp, fnsCell{0, "InSince", strconv.FormatInt(since+500, 10)}),
			window(c), c.left, "row 0 (parity-parent): InSince is"},
		"a start that is null": {fnsWith(t, c.resp, fnsCell{0, "InSince", "null"}), window(c), c.left,
			"row 0 (parity-parent): InSince is null"},
		"a child that left before its window": {c.resp, window(c), map[string][2]int64{childHost.Hostname: {child/1000 + 1,
			child/1000 + 2}}, "row 1 (parity-child): InSince is"},
		"a child that left after its window": {c.resp, window(c), map[string][2]int64{childHost.Hostname: {w[0] - 9,
			child/1000 - 1}}, "row 1 (parity-child): InSince is"},
		"a child that left without its row": {c.resp, window(c), map[string][2]int64{"parity-elsewhere": {0, 1 << 40}},
			"no row of parity-elsewhere, which left"},
		"no cell from the start": {fnsReplace(t, fnsReplace(t, c.resp, `"LOCALHOST"`, `"CONNECTED"`, 1), `"disabled"`,
			`"offline"`, -1), window(c), nil, "no cell reads the agent's start"},
	} {
		if got := problem(fnStreamingSince(fnsDoc(t, x.resp), x.started, x.left)); !strings.Contains(got, x.want) ||
			got == "" {
			t.Errorf("fnStreamingSince, %s: %q, want %q", name, got, x.want)
		}
	}
}

// testFnsRows: the oracle's guards (fnStreamingRows).
func testFnsRows(t *testing.T) {
	guard := fnStreamingGoneGuard(fnsGoneBase)
	for i, r := range fnsGone {
		if err := guard(fnsDoc(t, r.resp)); err != nil {
			t.Errorf("C's gone table %d: %v", i, err)
		}
	}
	for i, r := range fnsStandalone {
		if err := fnStreamingStandalone(fnsDoc(t, r.resp)); err != nil {
			t.Errorf("C's standalone table %d: %v", i, err)
		}
	}
	o := fnsGone[0].resp
	clock := fnsInt(t, fnsCellOf(t, o, 0, "dbTo"))
	for name, x := range map[string]struct{ resp, want string }{
		"the gone child still CONNECTED": {fnsWith(t, o, fnsCell{1, "InReason", `"CONNECTED"`}),
			`row 1: InReason is "CONNECTED", want "DISCONNECTED SOCKET CLOSED BY REMOTE END"`},
		"the refused child's reason the closed socket's": {fnsWith(t, o, fnsCell{2, "InReason",
			`"DISCONNECTED SOCKET CLOSED BY REMOTE END"`}), `row 2: InReason is`},
		"the gone child of severity normal": {fnsWith(t, o, fnsCell{1, "rowOptions", `{"severity":"normal"}`}),
			`row 1: rowOptions is {"severity":"normal"}`},
		"the gone child's dbTo the clock": {fnsWith(t, o, fnsCell{1, "dbTo", strconv.FormatInt(clock, 10)}), "row 1: dbTo is"},
		"the gone child still collected":  {fnsWith(t, o, fnsCell{1, "CollectedMetrics", "7"}), "row 1: CollectedMetrics is 7"},
		"the children in another order": {fnsReplace(t, fnsReplace(t, fnsReplace(t, o, `"parity-child"`, `"swap"`, 1),
			`"parity-child2"`, `"parity-child"`, 1), `"swap"`, `"parity-child2"`, 1), `row 1: Node is "parity-child2"`},
		"no table": {fnsReplace(t, o, `"type":"table"`, `"type":"topology"`, 1), `type is "topology"`},
	} {
		if err := guard(fnsDoc(t, x.resp)); err == nil || !strings.Contains(err.Error(), x.want) {
			t.Errorf("fnStreamingGoneGuard, %s: %v, want %q", name, err, x.want)
		}
	}
	one := fnsStandalone[0].resp
	if err := fnStreamingGoneGuard(fnsGoneBase)(fnsDoc(t, one)); err == nil || err.Error() != "1 rows, want 4" {
		t.Errorf("fnStreamingGoneGuard on one row: %v", err)
	}
	// the `calls` topology's guard (fnStreamingGuard) on C's table of H35's probe P1 (three of the 85 columns,
	// renumbered: dash.norm's `tight` rows hold the rest of it), and that table with its vnode taken for a local one
	calls := func(vnode string) Value {
		v, err := ParseJSON([]byte(`{"type":"table","columns":{"Node":{"index":0},"InStatus":{"index":1},` +
			`"InReason":{"index":2}},"data":[["parity-parent","initializing","LOCALHOST"],` +
			`["parity-rchild","online","CONNECTED"],["rchild-v","online","` + vnode + `"]]}`))
		if err != nil {
			t.Fatalf("harness: %v", err)
		}
		return v
	}
	if err := fnStreamingGuard(calls("CONNECTED")); err != nil {
		t.Errorf("fnStreamingGuard on C's calls table: %v", err)
	}
	if err := fnStreamingGuard(calls("VIRTUAL NODE")); err == nil || err.Error() !=
		`row 2: InReason is "VIRTUAL NODE", want "CONNECTED"` {
		t.Errorf("fnStreamingGuard, the vnode not on a receiver: %v", err)
	}
}

// testFnsMasks: the `gone` pair as fnStreamingRound compares it: the volatile columns' numbers other than 0 masked,
// but in the rows of the children that left, whose retention is compared (fnStreamingMasks), and each side's facts
// (fnStreamingFacts: an offline row's retention end is not the clock).
func testFnsMasks(t *testing.T) {
	diffs := func(o, c string) []string {
		var out []string
		ov, cv := fnsDoc(t, o), fnsDoc(t, c)
		for _, d := range Compare(ApplyMasks(ov, fnStreamingMasks(ov, fnStreamingVolatile)),
			ApplyMasks(cv, fnStreamingMasks(cv, fnStreamingVolatile))) {
			out = append(out, d.String())
		}
		return out
	}
	o, c := fnsGone[0], fnsGone[1]
	if d := diffs(o.resp, c.resp); d != nil {
		t.Errorf("C's gone tables differ: %q", d)
	}
	for i, r := range fnsGone {
		if err := fnStreamingFacts(fnsDoc(t, r.resp), r.port, r.flight); err != nil {
			t.Errorf("fnStreamingFacts on C's gone table %d: %v", i, err)
		}
	}
	cols := fnStreamingColumns(fnsDoc(t, c.resp))
	at := func(r int, col string) string { return fmt.Sprintf("$.data[%d][%d]:", r, cols[col]) }
	clock := fnsInt(t, fnsCellOf(t, c.resp, 0, "dbTo"))
	for name, x := range map[string]struct {
		resp string
		at   []string
	}{
		// rrdhost_retention()'s `now` for a host that is not online: its stored end is compared, not masked
		"the gone child's dbTo the clock": {fnsWith(t, c.resp, fnsCell{1, "dbTo", strconv.FormatInt(clock, 10)},
			fnsCell{1, "dbDuration", strconv.FormatInt((clock-fnsInt(t, fnsCellOf(t, c.resp, 1, "dbFrom")))/1000, 10)}),
			[]string{at(1, "dbTo"), at(1, "dbDuration")}},
		"the refused child's retention a second longer": {fnsWith(t, c.resp, fnsCell{2, "dbFrom",
			strconv.FormatInt(fnsInt(t, fnsCellOf(t, c.resp, 2, "dbFrom"))-1000, 10)}, fnsCell{2, "dbDuration",
			strconv.FormatInt(fnsInt(t, fnsCellOf(t, c.resp, 2, "dbDuration"))+1, 10)}),
			[]string{at(2, "dbFrom"), at(2, "dbDuration")}},
		// the Collected columns carry the db columns' maxima (function-netdata-streaming.c:706, :712, :718), not their
		// own cells' (0 once the children left)
		"the Collected maxima their own": {fnsWith(t, c.resp, fnsCell{-1, "CollectedMetrics", "0"},
			fnsCell{-1, "CollectedInstances", "0"}, fnsCell{-1, "CollectedContexts", "0"}),
			[]string{"$.columns.CollectedMetrics.max:", "$.columns.CollectedInstances.max:",
				"$.columns.CollectedContexts.max:"}},
		"the gone child still CONNECTED": {fnsWith(t, c.resp, fnsCell{1, "InReason", `"CONNECTED"`}),
			[]string{at(1, "InReason")}},
		"the refused child of severity normal": {fnsWith(t, c.resp, fnsCell{2, "rowOptions", `{"severity":"normal"}`}),
			[]string{"$.data[2][1].severity:"}},
		"the gone child's receiver's address kept": {fnsWith(t, c.resp, fnsCell{1, "InLocalIP", `"127.0.0.1"`}),
			[]string{at(1, "InLocalIP")}},
	} {
		d := diffs(o.resp, x.resp)
		ok := len(d) == len(x.at)
		for i := 0; ok && i < len(d); i++ {
			ok = strings.HasPrefix(d[i], x.at[i])
		}
		if !ok {
			t.Errorf("%s gives %q, want differences at %q", name, d, x.at)
		}
	}
	// an archived host is not online either (its InStatus): its stored retention is compared too
	archived := func(resp string, cells ...fnsCell) string {
		return fnsWith(t, resp, append([]fnsCell{{1, "InStatus", `"archived"`}}, cells...)...)
	}
	if d := diffs(archived(o.resp), archived(c.resp, fnsCell{1, "dbTo", strconv.FormatInt(clock, 10)})); len(d) != 1 ||
		!strings.HasPrefix(d[0], at(1, "dbTo")) {
		t.Errorf("an archived child's dbTo the clock gives %q, want a difference at %s", d, at(1, "dbTo"))
	}
	// the facts still hold an online host's retention end to the clock
	behind := fnsWith(t, c.resp, fnsCell{0, "dbTo", strconv.FormatInt(clock-1000, 10)})
	if err := fnStreamingFacts(fnsDoc(t, behind), "0", c.flight); err == nil ||
		!strings.Contains(err.Error(), "row 0: dbFrom 0 and dbTo") {
		t.Errorf("fnStreamingFacts, localhost's retention end a second behind: %v", err)
	}
}

// fnsClock is a recorded table's body with the spans of its clock's cells, to write it at another second (at): each
// number at [start, end) moves by scale for each second the clock moves.
type fnsClock struct {
	body  string
	clock int64
	spans []fnsSpan
}

type fnsSpan struct {
	start, end   int
	value, scale int64
}

// fnsClockOf is resp's body with its clock's cells: every age, the online rows' retention end and length (a stored
// retention does not move), each maximum that one of those cells is, the expiry.
func fnsClockOf(t *testing.T, resp string) fnsClock {
	t.Helper()
	_, body, _ := strings.Cut(resp, "\r\n\r\n")
	v := fnsDoc(t, resp)
	c := fnsClock{body: body, clock: fnsInt(t, fnsCellOf(t, resp, 0, "dbTo")) / 1000}
	cols := fnStreamingColumns(v)
	// add moves the number at path by scale a second; it hands the number back, if there is one
	add := func(path []string, scale int64) (int64, bool) {
		start, end, err := jsonSpanAt([]byte(body), path)
		if err != nil {
			t.Fatalf("harness: %v: %v", path, err)
		}
		n, err := strconv.ParseInt(body[start:end], 10, 64)
		if err != nil {
			return 0, false
		}
		c.spans = append(c.spans, fnsSpan{start, end, n, scale})
		return n, true
	}
	scales := map[string]int64{"InAge": 1, "OutAge": 1, "OutAttemptAge": 1, "dbTo": 1000, "dbDuration": 1}
	moving := map[string][]int64{}
	data, _ := dashMember(v, "data")
	for r := range data.Items {
		for col, scale := range scales {
			if (col == "dbTo" || col == "dbDuration") && fnStreamingOffline(v, r) {
				continue
			}
			if n, ok := add([]string{"data", "[" + strconv.Itoa(r) + "]", "[" + strconv.Itoa(cols[col]) + "]"}, scale); ok {
				moving[col] = append(moving[col], n)
			}
		}
	}
	for col, scale := range scales {
		m, err := dashAt(v, "columns", col, "max")
		if err != nil {
			t.Fatalf("harness: %v", err)
		}
		if n, err := strconv.ParseInt(m.Text, 10, 64); err == nil && slices.Contains(moving[col], n) {
			add([]string{"columns", col, "max"}, scale)
		}
	}
	add([]string{"expires"}, 1)
	slices.SortFunc(c.spans, func(a, b fnsSpan) int { return b.start - a.start })
	return c
}

// at is the body as the agent would write it at the second now.
func (c fnsClock) at(now int64) string {
	body := c.body
	for _, s := range c.spans {
		body = body[:s.start] + strconv.FormatInt(s.value+(now-c.clock)*s.scale, 10) + body[s.end:]
	}
	return body
}

// testFnsRound: fnStreamingRound's wiring, asked of two stub agents (dashNormStub) writing C's recorded `gone`
// tables at the second they answer in (fnsClock), each launched in its recorded second: C's pair holds; one thing
// wrong at a time is reported, on the oracle as the failure that ends the test.
func testFnsRound(t *testing.T) {
	// pair's candidate stub writes its Content-Length short bytes short of its body
	pairShort := func(o, c string, short int) *Pair {
		oc, cc := fnsClockOf(t, o), fnsClockOf(t, c)
		p := &Pair{Oracle: dashNormStub(t, stubAnswer{body: oc.at}.raw),
			Candidate: dashNormStub(t, stubAnswer{body: cc.at, short: short}.raw)}
		p.Oracle.LaunchStartedAt = time.Unix(fnsGone[0].launch, 0)
		p.Candidate.LaunchStartedAt = time.Unix(fnsGone[1].launch, 0)
		return p
	}
	pair := func(o, c string) *Pair { return pairShort(o, c, 0) }
	ask := fnStreamingAsk{target: "/api/v1/function?function=netdata-streaming", guard: fnStreamingGoneGuard(fnsGoneBase),
		left: [2]map[string][2]int64{fnsGone[0].left, fnsGone[1].left}}
	o, c := fnsGone[0].resp, fnsGone[1].resp
	if r := fnStreamingRound(pair(o, c), ask); r.fatal != "" || len(r.problems) > 0 {
		t.Errorf("C's gone tables: %q %q", r.fatal, r.problems)
	}
	child := fnsInt(t, fnsCellOf(t, c, 1, "InSince"))
	for name, x := range map[string]struct {
		o, c         string
		left         map[string][2]int64
		fatal, wants string
	}{
		"the candidate's gone child still CONNECTED": {o, fnsWith(t, c, fnsCell{1, "InReason", `"CONNECTED"`}), nil, "",
			"$.data[1]"},
		"the candidate's gone child left a second early": {o, fnsWith(t, c, fnsCell{1, "InSince",
			strconv.FormatInt(child-1000, 10)}, fnsCell{1, "InAge", strconv.FormatInt(fnsInt(t, fnsCellOf(t, c, 1,
			"InAge"))+1, 10)}), nil, "", "candidate: row 1 (parity-child): InSince is"},
		"the candidate's ages a second off": {o, fnsWith(t, c, fnsCell{1, "InAge", strconv.FormatInt(fnsInt(t,
			fnsCellOf(t, c, 1, "InAge"))+1, 10)}), nil, "", "candidate: row 1: InSince"},
		"the oracle's gone child of severity normal": {fnsWith(t, o, fnsCell{1, "rowOptions", `{"severity":"normal"}`}), c,
			nil, `oracle: row 1: rowOptions is {"severity":"normal"}`, ""},
		"the oracle's ages a second off": {fnsWith(t, o, fnsCell{2, "InAge", strconv.FormatInt(fnsInt(t,
			fnsCellOf(t, o, 2, "InAge"))+1, 10)}), c, nil, "oracle: row 2: InSince", ""},

		"a slash escaped": {o, fnsReplace(t, c, `"/etc/os-release"`, `"\/etc\/os-release"`, 1), nil, "", "escapes differ"},
		"minified":        {o, fnsReplace(t, c, "\n            [", "[", -1), nil, "", "layouts differ"},
		"the candidate's window not its own": {o, c, map[string][2]int64{childHost.Hostname: {child/1000 + 5,
			child/1000 + 9}, child2Host.Hostname: fnsGone[1].left[child2Host.Hostname]}, "",
			"candidate: row 1 (parity-child): InSince is"},
	} {
		a := ask
		if x.left != nil {
			a.left = [2]map[string][2]int64{fnsGone[0].left, x.left}
		}
		r := fnStreamingRound(pair(x.o, x.c), a)
		got := strings.Join(r.problems, "\n")
		switch {
		case x.fatal != "" && !strings.Contains(r.fatal, x.fatal):
			t.Errorf("%s: fatal %q, want %q", name, r.fatal, x.fatal)
		case x.fatal == "" && (r.fatal != "" || !strings.Contains(got, x.wants)):
			t.Errorf("%s: fatal %q, problems %q, want %q", name, r.fatal, got, x.wants)
		}
	}
	if r := fnStreamingRound(pairShort(o, c, 1), ask); r.fatal != "" || !strings.Contains(strings.Join(r.problems, "\n"),
		"candidate: Content-Length is") {
		t.Errorf("a length a byte short: fatal %q, problems %q", r.fatal, r.problems)
	}
}

// testFnsText: jsonSpanAt's steps, as fnStreamingMaskText writes a mask.
func testFnsText(t *testing.T) {
	doc := []byte("{\n    \"a\":[1, {\"b\":[2,\n  33]}],\n    \"c\":{\"[0]\":4}\n}")
	for path, want := range map[string]string{
		"a.[0]": "1", "a.[1].b.[1]": "33", "a.[1]": "{\"b\":[2,\n  33]}", "c": "{\"[0]\":4}",
	} {
		start, end, err := jsonSpanAt(doc, strings.Split(path, "."))
		if err != nil || string(doc[start:end]) != want {
			t.Errorf("jsonSpanAt %s: %q (%v), want %q", path, doc[start:end], err, want)
		}
	}
	for _, path := range []string{"a.[2]", "b", "a.b", "c.[0]"} {
		if _, _, err := jsonSpanAt(doc, strings.Split(path, ".")); err == nil {
			t.Errorf("jsonSpanAt %s: no error", path)
		}
	}
	got, err := fnStreamingMaskText(doc, []Mask{{"a.[1].b.[1]", "x"}, {"a.[0]", "y"}})
	if want := "{\n    \"a\":[<masked:y>, {\"b\":[2,\n  <masked:x>]}],\n    \"c\":{\"[0]\":4}\n}"; err != nil ||
		string(got) != want {
		t.Errorf("fnStreamingMaskText: %q (%v), want %q", got, err, want)
	}
}

// testFnsOut: the stream pairs' tables at each stage (fnStreamingOutGuard, fnStreamingOutFacts, the masks with
// fnStreamingOutVolatile), and fnStreamingRound's wiring of a topology's own masks and facts.
func testFnsOut(t *testing.T) {
	volatile := slices.Concat(fnStreamingVolatile, fnStreamingOutVolatile)
	diffs := func(o, c string) []string {
		var out []string
		ov, cv := fnsDoc(t, o), fnsDoc(t, c)
		for _, d := range Compare(ApplyMasks(ov, fnStreamingMasks(ov, volatile)), ApplyMasks(cv, fnStreamingMasks(cv, volatile))) {
			out = append(out, d.String())
		}
		return out
	}
	stages := map[string]struct {
		rec   [2]fnsRecorded
		sides [2]fnStreamingOutSide
	}{
		"never":     {fnsOutNever, fnsOutNeverSides},
		"connected": {fnsOutConnected, fnsOutConnectedSides},
		"denied":    {fnsOutDenied, fnsOutDeniedSides},
	}
	for stage, x := range stages {
		for i := range x.rec {
			v := fnsDoc(t, x.rec[i].resp)
			if err := fnStreamingOutGuard(stage, x.sides[0].stub)(v); err != nil {
				t.Errorf("%s: C's table %d: guard: %v", stage, i, err)
			}
			if err := fnStreamingOutFacts(v, x.sides[i]); err != nil {
				t.Errorf("%s: C's table %d: %v", stage, i, err)
			}
			if err := fnStreamingFacts(v, x.rec[i].port, x.rec[i].flight); err != nil {
				t.Errorf("%s: C's table %d: fnStreamingFacts: %v", stage, i, err)
			}
		}
		if d := diffs(x.rec[0].resp, x.rec[1].resp); d != nil {
			t.Errorf("%s: C's tables differ: %q", stage, d)
		}
	}
	if err := fnStreamingOutGuard("no-such-stage", "1")(fnsDoc(t, fnsOutNever[0].resp)); err == nil ||
		!strings.Contains(err.Error(), "harness: no stage") {
		t.Errorf("a stage the guard does not know: %v, want the harness's refusal", err)
	}
	cell := func(resp string, col string) int64 { return fnsInt(t, fnsCellOf(t, resp, 0, col)) }
	c, cs := fnsOutConnected[1], fnsOutConnectedSides[1]
	d, ds := fnsOutDenied[1], fnsOutDeniedSides[1]
	n, ns := fnsOutNever[1], fnsOutNeverSides[1]
	since := cell(c.resp, "OutAttemptSince")
	for name, x := range map[string]struct {
		resp string
		side fnStreamingOutSide
		want string
	}{
		"OutLocalPort with a max": {fnsReplace(t, c.resp, `"OutLocalPort":{`, `"OutLocalPort":{`+"\n"+`            "max":1,`, 1),
			cs, "columns.OutLocalPort has a max"},
		"OutLocalPort the parent's": {fnsWith(t, c.resp, fnsCell{0, "OutLocalPort", cs.stub}), cs, "row 0: OutLocalPort"},
		"OutLocalPort the agent's":  {fnsWith(t, c.resp, fnsCell{0, "OutLocalPort", cs.listen}), cs, "row 0: OutLocalPort"},
		"OutLocalPort kept once refused": {fnsWith(t, d.resp, fnsCell{0, "OutLocalPort", fnsCellOf(t, c.resp, 0,
			"OutLocalPort")}), ds, "row 0: OutLocalPort " + fnsCellOf(t, c.resp, 0, "OutLocalPort")},
		"the attempt in seconds": {fnsWith(t, c.resp, fnsCell{0, "OutAttemptSince", strconv.FormatInt(since/1000, 10)},
			fnsCell{-1, "OutAttemptSince", strconv.FormatInt(since/1000, 10)}), cs, "row 0: OutAttemptSince"},
		"the attempt's age rounded up": {fnsWith(t, c.resp, fnsCell{0, "OutAttemptAge",
			strconv.FormatInt(cell(c.resp, "OutAttemptAge")+1, 10)}, fnsCell{-1, "OutAttemptAge",
			strconv.FormatInt(cell(c.resp, "OutAttemptAge")+1, 10)}), cs, "row 0: OutAttemptSince"},
		"the attempt without its age": {fnsWith(t, c.resp, fnsCell{0, "OutAttemptAge", "null"}), cs, "row 0: OutAttemptSince"},
		"an attempt's age without its time": {fnsWith(t, c.resp, fnsCell{0, "OutAttemptSince", "null"}), cs,
			"row 0: OutAttemptSince"},
		"the refusal's attempt the connection's": {fnsWith(t, d.resp, fnsCell{0, "OutAttemptSince",
			strconv.FormatInt(since, 10)}, fnsCell{0, "OutAttemptAge", strconv.FormatInt(
			cell(d.resp, "OutAttemptAge")+cell(d.resp, "OutAttemptSince")/1000-since/1000, 10)},
			fnsCell{-1, "OutAttemptSince", strconv.FormatInt(since, 10)}), ds, "row 0: OutAttemptSince"},
		"OutSince the refusal's second": {fnsWith(t, d.resp, fnsCell{0, "OutSince", strconv.FormatInt(ds.closed[0]*1000, 10)},
			fnsCell{0, "OutAge", strconv.FormatInt(cell(d.resp, "OutAge")-(ds.closed[0]-cell(d.resp, "OutSince")/1000), 10)}),
			ds, "row 0: OutSince"},
		"the parents' list made before the launch": {fnsWith(t, n.resp, fnsCell{0, "OutAttemptSince",
			strconv.FormatInt((ns.started[0]-1)*1000, 10)}, fnsCell{0, "OutAttemptAge", strconv.FormatInt(
			cell(n.resp, "dbTo")/1000-ns.started[0]+1, 10)}), ns, "row 0: OutAttemptSince"},
		"OutSince the clock before any connection": {fnsWith(t, n.resp, fnsCell{0, "OutSince",
			strconv.FormatInt(cell(n.resp, "dbTo"), 10)}, fnsCell{0, "OutAge", "0"}), ns, "row 0: OutSince"},
		"a traffic max not the largest": {fnsWith(t, c.resp, fnsCell{-1, "OutTrafficData", "1"}), cs,
			"columns.OutTrafficData.max is 1"},
		"the Collected maxima their own": {fnsWith(t, c.resp, fnsCell{-1, "CollectedMetrics", "0"}), cs,
			"columns.CollectedMetrics.max is 0, the db column's largest cell"},
		"a count that is null": {fnsWith(t, c.resp, fnsCell{0, "OutTrafficFunctions", "null"}), cs,
			"row 0: OutTrafficFunctions is null"},
	} {
		if err := fnStreamingOutFacts(fnsDoc(t, x.resp), x.side); err == nil || !strings.Contains(err.Error(), x.want) {
			t.Errorf("fnStreamingOutFacts, %s: %v, want %q", name, err, x.want)
		}
	}
	// a Collected count below the db's keeps the db column's maximum (function-netdata-streaming.c:706): C's rule holds
	low := fnsWith(t, c.resp, fnsCell{0, "CollectedMetrics", strconv.FormatInt(cell(c.resp, "CollectedMetrics")-7, 10)})
	if err := fnStreamingOutFacts(fnsDoc(t, low), cs); err != nil {
		t.Errorf("fnStreamingOutFacts, a Collected count below the db's: %v", err)
	}
	// what the comparison hides and what it compares, each at one more than C's number in the candidate's cell and
	// maximum: hidden, the columns probe p1b showed differing C against C (fnStreamingOutVolatile); compared, the
	// counts and the metadata and functions bytes, equal on every C side
	for _, x := range []struct {
		names  []string
		masked bool
	}{
		{[]string{"OutLocalPort", "OutTrafficData", "OutTrafficReplication", "OutAttemptSince", "OutAttemptAge"}, true},
		{[]string{"dbMetrics", "dbInstances", "dbContexts", "CollectedMetrics", "CollectedInstances", "CollectedContexts",
			"OutTrafficMetadata", "OutTrafficFunctions"}, false},
	} {
		for _, name := range x.names {
			v := cell(c.resp, name)
			cells := []fnsCell{{0, name, strconv.FormatInt(v+1, 10)}}
			if name != "OutLocalPort" {
				cells = append(cells, fnsCell{-1, name, strconv.FormatInt(v+1, 10)})
			}
			if d := diffs(fnsOutConnected[0].resp, fnsWith(t, c.resp, cells...)); (d == nil) != x.masked {
				t.Errorf("%s one more than C's: %q, want it masked: %t", name, d, x.masked)
			}
		}
	}
	// the stage's guard refuses a table of another state
	for name, x := range map[string]struct {
		stage, resp, want string
	}{
		"never: a reason of a connection": {"never", fnsWith(t, fnsOutNever[0].resp, fnsCell{0, "OutReason", `"CONNECTED"`}),
			`row 0: OutReason is "CONNECTED", want "NEVER CONNECTED"`},
		"never: the ends of no sender": {"never", fnsWith(t, fnsOutNever[0].resp, fnsCell{0, "OutLocalIP", `""`}),
			`row 0: OutLocalIP is "", want "not connected"`},
		"connected: another parent's port": {"connected", fnsWith(t, fnsOutConnected[0].resp, fnsCell{0, "OutRemotePort",
			"1"}), "row 0: OutRemotePort is 1, want " + fnsOutConnectedSides[0].stub},
		"connected: the handshake's reason": {"connected", fnsWith(t, fnsOutConnected[0].resp, fnsCell{0,
			"OutAttemptHandshake", `["CONNECTED"]`}), `row 0: OutAttemptHandshake is ["CONNECTED"]`},
		"denied: replication complete": {"denied", fnsWith(t, fnsOutDenied[0].resp, fnsCell{0, "OutReplCompletion", "100"}),
			"row 0: OutReplCompletion is 100, want 0"},
		"denied: still connected": {"denied", fnsWith(t, fnsOutDenied[0].resp, fnsCell{0, "OutStatus", `"online"`}),
			`row 0: OutStatus is "online", want "offline"`},
	} {
		if err := fnStreamingOutGuard(x.stage, fnsOutConnectedSides[0].stub)(fnsDoc(t, x.resp)); err == nil ||
			!strings.Contains(err.Error(), x.want) {
			t.Errorf("fnStreamingOutGuard, %s: %v, want %q", name, err, x.want)
		}
	}
	// what the comparison sees: the candidate's recorded table with one thing wrong
	o := fnsOutConnected[0].resp
	for name, x := range map[string]struct {
		o, c string
		at   string
	}{
		"connected: compressed":         {o, fnsWith(t, c.resp, fnsCell{0, "OutCompression", `"COMPRESSED"`}), "$.data[0]"},
		"connected: no metadata sent":   {o, fnsWith(t, c.resp, fnsCell{0, "OutTrafficMetadata", "0"}), "$.data[0]"},
		"connected: another capability": {o, fnsReplace(t, c.resp, `"FLOATBASELINE"]`, `"ML"]`, 1), "$.data[0]"},
		"denied: the reason CONNECTED": {fnsOutDenied[0].resp, fnsWith(t, d.resp, fnsCell{0, "OutReason", `"CONNECTED"`}),
			"$.data[0]"},
		"denied: the traffic zeroed": {fnsOutDenied[0].resp, fnsWith(t, d.resp, fnsCell{0, "OutTrafficData", "0"}),
			"$.data[0]"},
		"denied: replication complete": {fnsOutDenied[0].resp, fnsWith(t, d.resp, fnsCell{0, "OutReplCompletion", "100"}),
			"$.data[0]"},
		"never: severity normal": {fnsOutNever[0].resp, fnsWith(t, n.resp, fnsCell{0, "rowOptions",
			`{"severity":"normal"}`}), "$.data[0][1]"},
		"never: the ends of no sender": {fnsOutNever[0].resp, fnsWith(t, n.resp, fnsCell{0, "OutLocalIP", `""`}), "$.data[0]"},
		"never: the attempt never made": {fnsOutNever[0].resp, fnsWith(t, n.resp, fnsCell{0, "OutAttemptHandshake", `[]`}),
			"$.data[0]"},
	} {
		if d := diffs(x.o, x.c); len(d) == 0 || !strings.HasPrefix(d[0], x.at) {
			t.Errorf("%s gives %q, want a difference at %s", name, d, x.at)
		}
	}
	// the round applies a topology's own masks and facts: C's connected pair holds; a sender's port that is the
	// parent's is reported; the oracle's guard ends it
	pair := func(o, c string) *Pair {
		oc, cc := fnsClockOf(t, o), fnsClockOf(t, c)
		p := &Pair{Oracle: dashNormStub(t, stubAnswer{body: oc.at}.raw),
			Candidate: dashNormStub(t, stubAnswer{body: cc.at}.raw)}
		p.Oracle.LaunchStartedAt = time.Unix(fnsOutConnected[0].launch, 0)
		p.Candidate.LaunchStartedAt = time.Unix(fnsOutConnected[1].launch, 0)
		return p
	}
	sides := fnsOutConnectedSides
	ask := fnStreamingAsk{target: "/api/v1/function?function=netdata-streaming",
		guard: fnStreamingOutGuard("connected", sides[0].stub), volatile: fnStreamingOutVolatile,
		facts: func(i int, v Value, flight [2]int64) error { return fnStreamingOutFacts(v, sides[i]) }}
	if r := fnStreamingRound(pair(o, c.resp), ask); r.fatal != "" || len(r.problems) > 0 {
		t.Errorf("C's connected tables through the round: %q %q", r.fatal, r.problems)
	}
	bad := fnsWith(t, c.resp, fnsCell{0, "OutLocalPort", cs.stub})
	if r := fnStreamingRound(pair(o, bad), ask); r.fatal != "" || !strings.Contains(strings.Join(r.problems, "\n"),
		"candidate: row 0: OutLocalPort") {
		t.Errorf("the round, a sender's port that is the parent's: %q %q", r.fatal, r.problems)
	}
	if r := fnStreamingRound(pair(fnsWith(t, o, fnsCell{0, "OutStatus", `"replicating"`}), c.resp), ask); !strings.Contains(
		r.fatal, `oracle: row 0: OutStatus is "replicating"`) {
		t.Errorf("the round, the oracle replicating: %q", r.fatal)
	}
}
