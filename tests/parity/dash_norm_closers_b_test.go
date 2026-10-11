// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"math"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The pins of set B's rows (nodes_stream_setb_test.go; check `api.v2-node-instances-setb`, and fn.stream's
// `node-instances`), daemon-free on the answers C gave in SA-B's recording run (dash_norm_closers_b_data_test.go): C's
// double text and the completions (setbFormat, setbCompletion), each render on hand-made inputs, then every recorded
// row: C's pair judged as a live round judges it (v2Judge; the table's guard, facts and masks), and named wrong
// candidates, each one thing wrong in the candidate's (or, for a guard, the oracle's) recorded answer; and the
// table's round (fnStreamingRound) asked of two stub agents.

// setbRec is a recorded row: each side's raw answer (0 the oracle's; its run directory written <run>) and the seconds
// its request was in flight.
type setbRec struct {
	raw    [2]string
	flight [2][2]int64
}

// setbReplRecord is a recorded stage of TestNodeInstancesSetB/replicating.
type setbReplRecord struct {
	base     int64
	launches [2]int64
	sides    [2]setbReplSide
	ni, fn   setbRec
}

// setbRelayRecord is a recorded stage of TestNodeInstancesSetB/relay.
type setbRelayRecord struct {
	base     int64
	port     string
	launches [2]int64
	sides    [2]setbRelaySide
	ni, fn   setbRec
}

// setbVnodeRecord is the recorded TestNodeInstancesSetB/vnode.
type setbVnodeRecord struct {
	firsts   [2][2]int64
	launches [2]int64
	sides    [2]niSide
	ni, fn   setbRec
}

// setbArchivedRecord is the recorded TestNodeInstancesSetB/archived.
type setbArchivedRecord struct {
	launches [2]int64
	sides    [2]niSide
	ni, fn   setbRec
}

// setbClaimRecord is a recorded stage of TestNodeInstancesSetB/claim.
type setbClaimRecord struct {
	launches [2]int64
	sides    [2]setbClaimSide
	ni, fn   setbRec
}

// setbFnStreamRecord is the recorded TestFnStream/calls/node-instances.
type setbFnStreamRecord struct {
	launches [2]int64
	sides    [2]niSide
	ni       setbRec
}

// setbWithBody is raw with its body replaced by body and its Content-Length the new body's.
func setbWithBody(t *testing.T, raw, body string) string {
	t.Helper()
	head, _, ok := strings.Cut(raw, "\r\n\r\n")
	if !ok {
		t.Fatalf("harness: no head in %q", truncateBytes([]byte(raw)))
	}
	head = contentLengthRe.ReplaceAllString(head, "Content-Length: "+strconv.Itoa(len(body)))
	return head + "\r\n\r\n" + body
}

// setbPlant is raw with the value at path (from the body's root) written as value (niStreamEdit), its length fixed.
func setbPlant(t *testing.T, raw string, path []string, value string) string {
	t.Helper()
	return setbWithBody(t, raw, niStreamEdit(t, string(httpBody([]byte(raw))), path, value, "", false))
}

// setbJudgeNI judges a recorded node-instance pair with the candidate's answer cand as v2Round judges a live round
// (v2Judge), the network aside: the problems.
func setbJudgeNI(req v2Req, fam v2Family, rec setbRec, oracle, cand string) []string {
	a := v2Answer{raw: [2][]byte{[]byte(oracle), []byte(cand)}, flight: rec.flight}
	v2Judge(&a, req, fam, [2]string{string(Oracle), string(Candidate)}, [2]string{})
	return a.problems
}

// setbNICase is a named wrong answer of a recorded node-instance row: the candidate's (or with oracle, the oracle's)
// value at path written as value, and a text the problems must hold (with oracle, a problem must start with it: the
// guard's are written `oracle: <fact>`, where a layout's difference also quotes the oracle's body after "oracle:").
type setbNICase struct {
	name   string
	path   []string
	value  string
	oracle bool
	want   string
}

// setbHas tells whether one of the problems is the case's: for the oracle's, one that starts with want.
func (c setbNICase) setbHas(problems []string) bool {
	return slices.ContainsFunc(problems, func(s string) bool {
		if c.oracle {
			return strings.HasPrefix(s, c.want)
		}
		return strings.Contains(s, c.want)
	})
}

// setbCheckNI pins a recorded node-instance row: C's pair shows no problem, and each case's planted answer one that
// holds its text (setbHas).
func setbCheckNI(t *testing.T, key string, req v2Req, fam v2Family, rec setbRec, cases []setbNICase) {
	t.Helper()
	if p := setbJudgeNI(req, fam, rec, rec.raw[0], rec.raw[1]); len(p) > 0 {
		t.Errorf("%s: C's pair: %q", key, p)
	}
	for _, c := range cases {
		o, cand := rec.raw[0], rec.raw[1]
		if c.oracle {
			o = setbPlant(t, o, c.path, c.value)
		} else {
			cand = setbPlant(t, cand, c.path, c.value)
		}
		if p := setbJudgeNI(req, fam, rec, o, cand); !c.setbHas(p) {
			t.Errorf("%s, %s: problems %q, want one with %q", key, c.name, p, c.want)
		}
	}
}

// setbFnCase is a named wrong table: row r's cell of col (r -1: the column's max) written as text in the candidate's
// (or with oracle, the oracle's) recorded table, and a text the judgement must hold.
type setbFnCase struct {
	name   string
	cells  []fnsCell
	oracle bool
	want   string
}

// setbJudgeFn judges a recorded table pair with ask's own parts, the way fnStreamingRound judges a round once it has
// the answers (its exchange and head comparison aside: those are pinned by testFnsRound and by testDashNormClosersB's
// stub rounds): the oracle's guard, each side's fnStreamingFacts (its port and flight), fnStreamingSince (its start
// window, launch to launch + niStartSlack) and ask.facts, then the bodies with fnStreamingVolatile and ask.volatile's
// numbers masked. ports and launches are each side's.
func setbJudgeFn(t *testing.T, ask fnStreamingAsk, rec setbRec, ports [2]string, launches [2]int64, raw [2]string) []string {
	t.Helper()
	var doc [2]Value
	var problems []string
	for i := range raw {
		_, v, err := fnStreamingDoc([]byte(raw[i]))
		if err != nil {
			t.Fatalf("harness: %v", err)
		}
		doc[i] = v
	}
	if err := ask.guard(doc[0]); err != nil {
		problems = append(problems, "oracle: guard: "+err.Error())
	}
	for i := range doc {
		for _, err := range []error{fnStreamingFacts(doc[i], ports[i], rec.flight[i]),
			fnStreamingSince(doc[i], [2]int64{launches[i], launches[i] + niStartSlack}, ask.left[i]),
			ask.facts(i, doc[i], rec.flight[i])} {
			if err != nil {
				problems = append(problems, string([]Role{Oracle, Candidate}[i])+": "+err.Error())
			}
		}
	}
	volatile := slices.Concat(fnStreamingVolatile, ask.volatile)
	for _, d := range Compare(ApplyMasks(doc[0], fnStreamingMasks(doc[0], volatile)),
		ApplyMasks(doc[1], fnStreamingMasks(doc[1], volatile))) {
		problems = append(problems, d.String())
	}
	return problems
}

// setbCheckFn pins a recorded table row: C's pair shows no problem, each case's planted table one that holds its
// text.
func setbCheckFn(t *testing.T, key string, ask fnStreamingAsk, rec setbRec, ports [2]string, launches [2]int64,
	cases []setbFnCase) {
	t.Helper()
	if p := setbJudgeFn(t, ask, rec, ports, launches, rec.raw); len(p) > 0 {
		t.Errorf("%s: C's tables: %q", key, p)
	}
	for _, c := range cases {
		raw := rec.raw
		k := 1
		if c.oracle {
			k = 0
		}
		raw[k] = fnsWith(t, raw[k], c.cells...)
		p := setbJudgeFn(t, ask, rec, ports, launches, raw)
		if !slices.ContainsFunc(p, func(s string) bool { return strings.Contains(s, c.want) }) {
			t.Errorf("%s, %s: problems %q, want one with %q", key, c.name, p, c.want)
		}
	}
}

// setbStubPair is a pair of stub agents (dashNormStub) answering the recorded tables at the second they are asked
// (fnsClockOf), each launched in its recorded second, each table's inbound local ports (of a connected row) written
// as its stub's port: what fnStreamingFacts holds them to.
func setbStubPair(t *testing.T, raw [2]string, launches [2]int64) *Pair {
	t.Helper()
	p := &Pair{}
	for i := range raw {
		var clock fnsClock
		d := dashNormStub(t, func() string { return stubAnswer{body: clock.at}.raw() })
		_, port, _ := strings.Cut(d.Addr, ":")
		resp := raw[i]
		v := fnsDoc(t, resp)
		data, _ := dashMember(v, "data")
		var cells []fnsCell
		for r := range data.Items {
			if c, err := fnStreamingCell(v, r, "InReason"); err == nil && c.Text == "CONNECTED" {
				cells = append(cells, fnsCell{r, "InLocalPort", port})
			}
		}
		if len(cells) > 0 {
			cells = append(cells, fnsCell{-1, "InLocalPort", port})
		}
		clock = fnsClockOf(t, fnsWith(t, resp, cells...))
		d.LaunchStartedAt = time.Unix(launches[i], 0)
		if i == 0 {
			p.Oracle = d
		} else {
			p.Candidate = d
		}
	}
	return p
}

// testDashNormClosersBFormat pins C's double text and the two completions.
func testDashNormClosersBFormat(t *testing.T) {
	for v, want := range map[float64]string{
		100: "100", 0: "0", 0.8: "0.8", 2.5: "2.5", -2.5: "-2.5", 1e-7: "0.0000001", 4e-8: "0", 0.99999996: "1",
		18.181818181818183: "18.1818182", 17.24137931034483: "17.2413793", 12345678.123456789: "12345678.1234568",
		math.NaN(): "null", math.Inf(1): "null", math.Inf(-1): "null",
	} {
		if got := setbFormat(v); got != want {
			t.Errorf("setbFormat(%v) = %q, want %q", v, got, want)
		}
	}
	for _, c := range []struct {
		started, current, now int64
		want                  string
	}{
		// C's receiver in probe p1: base 1791639960, the partial answer to base+30 taken at 1791640125
		{1791639960, 1791639990, 1791640125, "18.1818182"},
		// C's sender: X asked from base-60 = 1791636420, answered to base-30, the status read at 1791640164
		{1791636420, 1791636450, 1791640164, "0.8012821"},
		{10, 10, 10, "null"}, {10, 20, 10, "null"}, {10, 20, 30, "50"}, {10, 30, 20, "200"},
	} {
		if got := setbCompletion(c.started, c.current, c.now); got != c.want {
			t.Errorf("setbCompletion(%d, %d, %d) = %q, want %q", c.started, c.current, c.now, got, c.want)
		}
	}
}

// testDashNormClosersBRenders pins the renders on hand-made inputs: the partial completion of each second of the
// answer, the sender's completion at the walk's clock, the stream path's start, the first stored points; and the
// window and stored-end facts.
func testDashNormClosersBRenders(t *testing.T) {
	for _, c := range []struct {
		ms   int64
		w    [2]int64
		want bool
	}{{1000, [2]int64{1, 1}, true}, {2000, [2]int64{1, 2}, true}, {1500, [2]int64{1, 1}, false},
		{999, [2]int64{0, 1}, false}, {3000, [2]int64{1, 2}, false}, {0, [2]int64{1, 2}, false}} {
		if got := setbWithin(c.ms, c.w); got != c.want {
			t.Errorf("setbWithin(%d, %v) = %v, want %v", c.ms, c.w, got, c.want)
		}
	}
	side := setbReplSide{started: 100, current: 130, answered: [2]int64{160, 161}}
	if got, want := side.partials(), []string{"50", "49.1803279"}; !slices.Equal(got, want) {
		t.Errorf("partials = %q, want %q", got, want)
	}
	if got := (setbReplSide{started: 100, current: 130}).partials(); got != nil {
		t.Errorf("partials before the answer = %q", got)
	}
	for in, want := range map[string]string{
		`{"completion":50,"x":1}`:    `{"completion":"PARTIAL","x":1}`,
		`{"completion": 49.1803279}`: `{"completion": "PARTIAL"}`,
		`{"completion":100}`:         `{"completion":100}`,
		`{"completion":49.18}`:       `{"completion":49.18}`,
		`{"completion":null}`:        `{"completion":null}`,
		`{"completion":"50"}`:        `{"completion":"50"}`,
	} {
		if got := string(setbPartialRender(side.partials(), []byte(in))); got != want {
			t.Errorf("setbPartialRender(%s) = %s, want %s", in, got, want)
		}
	}
	relay := setbRelaySide{niStreamSide: niStreamSide{niSide: niSide{opened: [2]int64{140, 141}}}, oldest: 100,
		latest: 130}
	if got := relay.completion(160); got != "50" {
		t.Errorf("completion(160) = %q", got)
	}
	if got := (setbRelaySide{}).completion(160); got != "" {
		t.Errorf("completion without stamps = %q", got)
	}
	obj := `{"replication":{"completion":50},"destination":{"streaming_path":[{"since":140,"x":141},{"since":142}],` +
		`"completion":50}}`
	for _, c := range []struct {
		now     int64
		clocked bool
		want    string
	}{
		{160, true, `{"replication":{"completion":"COMPLETION"},"destination":{"streaming_path":[{"since":"OPENED",` +
			`"x":141},{"since":142}],"completion":50}}`},
		{161, true, `{"replication":{"completion":50},"destination":{"streaming_path":[{"since":"OPENED","x":141},` +
			`{"since":142}],"completion":50}}`},
		{160, false, `{"replication":{"completion":50},"destination":{"streaming_path":[{"since":"OPENED","x":141},` +
			`{"since":142}],"completion":50}}`},
	} {
		if got := string(setbRelayWords(relay, []byte(obj), c.now, c.clocked)); got != c.want {
			t.Errorf("setbRelayWords at %d (%v) = %s\nwant %s", c.now, c.clocked, got, c.want)
		}
	}
	vnode := setbVnodeFamily([2]niSide{{listen: "1"}, {listen: "1"}}, [2][2]int64{{200, 205}, {201, 206}})
	for i, want := range []string{`{"first_time":"VNODE-FIRST","a":{"first_time":"LOCAL-FIRST"},"b":{"first_time":201}}`,
		`{"first_time":200,"a":{"first_time":205},"b":{"first_time":"VNODE-FIRST"}}`} {
		in := `{"first_time":200,"a":{"first_time":205},"b":{"first_time":201}}`
		if got := string(vnode.render(i, [2]int64{}, []byte(in))); got != want {
			t.Errorf("setbVnodeFamily's render, side %d: %s\nwant %s", i, got, want)
		}
	}
	claim := setbClaimFamily([2]setbClaimSide{{niSide: niSide{listen: "1"}, child: [2]int64{300, 301}},
		{niSide: niSide{listen: "1"}, child: [2]int64{310, 310}}})
	for i, want := range []string{`{"a":{"first_time":295},"b":{"first_time":"CHILD-FIRST"},"c":{"first_time":"CHILD-FIRST"},` +
		`"d":{"first_time":298},"e":{"first_time":306}}`,
		`{"a":{"first_time":295},"b":{"first_time":296},"c":{"first_time":297},"d":{"first_time":298},` +
			`"e":{"first_time":"CHILD-FIRST"}}`} {
		in := `{"a":{"first_time":295},"b":{"first_time":296},"c":{"first_time":297},"d":{"first_time":298},"e":{"first_time":306}}`
		if got := string(claim.render(i, [2]int64{}, []byte(in))); got != want {
			t.Errorf("setbClaimFamily's render, side %d: %s\nwant %s", i, got, want)
		}
	}
	if (setbClaimSide{}).childFirst(-4) || (setbClaimSide{}).childFirst(0) {
		t.Errorf("childFirst holds with no child clock")
	}
	db, ingest := []string{"db"}, []string{"ingest"}
	for in, ok := range map[string]bool{
		`{"db":{"first_time":10,"last_time":12},"ingest":{"since":12}}`:      true,
		`{"db":{"first_time":10,"last_time":12},"ingest":{"since":10}}`:      false,
		`{"db":{"first_time":10,"last_time":12},"ingest":{"since":13}}`:      false,
		`{"db":{"first_time":10,"last_time":12},"ingest":{"since":"START"}}`: false,
		`{"db":{"first_time":12,"last_time":12},"ingest":{"since":12}}`:      false,
		`{"db":{"first_time":0,"last_time":12},"ingest":{"since":12}}`:       false,
		`{"db":{"first_time":10,"last_time":"NOW"},"ingest":{"since":12}}`:   false,
	} {
		v, err := ParseJSON([]byte(in))
		if err != nil {
			t.Fatal(err)
		}
		if err := setbArchivedSince(db, ingest)(v); (err == nil) != ok {
			t.Errorf("setbArchivedSince(%s): %v, want it to hold: %v", in, err, ok)
		}
	}
	for in, ok := range map[string]bool{`{"x":12}`: true, `{"x":"NOW"}`: false, `{"x":null}`: false} {
		v, err := ParseJSON([]byte(in))
		if err != nil {
			t.Fatal(err)
		}
		if err := setbStoredEnd("x")(v); (err == nil) != ok {
			t.Errorf("setbStoredEnd(%s): %v, want it to hold: %v", in, err, ok)
		}
	}
	fns := fnStreamNIFamily([2]niSide{{listen: "1", opened: [2]int64{300, 310}}, {listen: "1", opened: [2]int64{300, 310}}})
	in := `{"a":{"first_time":299},"b":{"first_time":300},"c":{"first_time":310},"d":{"first_time":311},"e":{"first_time":0}}`
	want := `{"a":{"first_time":299},"b":{"first_time":"CHILD-FIRST"},"c":{"first_time":"CHILD-FIRST"},` +
		`"d":{"first_time":311},"e":{"first_time":0}}`
	if got := string(fns.render(0, [2]int64{}, []byte(in))); got != want {
		t.Errorf("fnStreamNIFamily's render: %s\nwant %s", got, want)
	}
}

// setbPortsOf are a record's sides' listening ports.
func setbPortsOf(sides [2]niSide) [2]string { return [2]string{sides[0].listen, sides[1].listen} }

// testDashNormClosersBRepl pins TestNodeInstancesSetB/replicating's rows on C's recorded stages.
func testDashNormClosersBRepl(t *testing.T) {
	if got := len(setbReplRecorded); got != len(setbReplStages) {
		t.Fatalf("%d recorded stages, want %d", got, len(setbReplStages))
	}
	ingest := []string{"nodes", "[0]", "instances", "[0]", "ingest"}
	at := func(keys ...string) []string { return append(slices.Clone(ingest), keys...) }
	for _, stage := range setbReplStages {
		rec := setbReplRecorded[stage]
		key := "replicating/" + stage
		cases := []setbNICase{
			// the plan's m27 (the completion always 100) and a completion of another second than the answer's
			{"the completion 100", at("replication", "completion"), "100", false, ".completion"},
			// m28 (in progress whenever the status loaded the charts) and m29 (printed only while charts replicate)
			{"in progress", at("replication", "in_progress"), "true", false, ".in_progress"},
			{"no replication", at("replication"), "", false, ".ingest.<members>"},
			{"online", at("status"), `"online"`, false, ".status"},
			{"no source", at("source"), "", false, ".ingest.<members>"},
			{"the connection's count 0", at("id"), "0", false, ".id"},
			{"the oracle online", at("status"), `"online"`, true, "oracle:"},
			{"the oracle's replication 100 in progress", at("replication"),
				`{"in_progress":true,"completion":100,"instances":0}`, true, "oracle:"},
		}
		if stage == "open" {
			cases[0] = setbNICase{"the completion 0", at("replication", "completion"), "0", false, ".completion"}
			cases[1] = setbNICase{"not in progress", at("replication", "in_progress"), "false", false, ".in_progress"}
		}
		if stage == "partial" {
			cases[1] = setbNICase{"not in progress", at("replication", "in_progress"), "false", false, ".in_progress"}
			cases = append(cases, setbNICase{"the completion a second later",
				at("replication", "completion"), setbCompletion(rec.sides[1].started, rec.sides[1].current,
					rec.sides[1].answered[1]+1), false, ".completion"})
		}
		setbCheckNIDrop(t, key, setbReplRow(stage, rec.base), setbReplFamily(rec.sides), rec.ni, cases)
		ask := setbReplAsk(stage, rec.sides)
		var ports [2]string
		for i := range rec.sides {
			ports[i] = rec.sides[i].listen
		}
		child := setbRow(fnsDoc(t, rec.fn.raw[0]), setbReplHost.Hostname)
		fcases := []setbFnCase{
			{"the child online", []fnsCell{{child, "InStatus", `"online"`}}, false, ".data[1]"},
			{"no instance replicating", []fnsCell{{child, "InReplInstances", "7"}}, false, ".data[1]"},
			{"the oracle's child online", []fnsCell{{child, "InStatus", `"online"`}}, true, "oracle: guard"},
			// a second before the connection, its age that much longer: one clock still (fnStreamingFacts holds)
			{"the child's InSince before its connection", setbMoveTo(t, rec.fn.raw[1], child, "InSince", "InAge",
				rec.sides[1].opened[0]-1), false, "candidate: row 1: InSince"},
		}
		if stage != "open" {
			fcases = append(fcases,
				setbFnCase{"the completion 100 (m27)", []fnsCell{{child, "InReplCompletion", "100"}}, false,
					"candidate: row 1: InReplCompletion"},
				setbFnCase{"the oracle's completion 100", []fnsCell{{child, "InReplCompletion", "100"}}, true,
					"oracle: guard"},
				setbFnCase{"the completion's max 18", []fnsCell{{-1, "InReplCompletion", "18"}}, false,
					"columns.InReplCompletion.max"})
		} else {
			fcases = append(fcases,
				setbFnCase{"the completion 0", []fnsCell{{child, "InReplCompletion", "0"}}, false, ".data[1]"},
				setbFnCase{"the oracle's completion 0", []fnsCell{{child, "InReplCompletion", "0"}}, true,
					"oracle: guard"})
		}
		setbCheckFn(t, key, ask, rec.fn, ports, rec.launches, fcases)
	}
}

// setbCheckNIDrop is setbCheckNI where a case's empty value drops the member at its path.
func setbCheckNIDrop(t *testing.T, key string, req v2Req, fam v2Family, rec setbRec, cases []setbNICase) {
	t.Helper()
	var kept []setbNICase
	for _, c := range cases {
		if c.value != "" {
			kept = append(kept, c)
			continue
		}
		o, cand := rec.raw[0], rec.raw[1]
		edit := func(raw string) string {
			return setbWithBody(t, raw, niStreamEdit(t, string(httpBody([]byte(raw))), c.path, "", "", true))
		}
		if c.oracle {
			o = edit(o)
		} else {
			cand = edit(cand)
		}
		if p := setbJudgeNI(req, fam, rec, o, cand); !c.setbHas(p) {
			t.Errorf("%s, %s: problems %q, want one with %q", key, c.name, p, c.want)
		}
	}
	setbCheckNI(t, key, req, fam, rec, kept)
}

// setbEarlier are the cells of row r of a recorded table that move the time in col a second earlier and its age (in
// age) a second longer, so that the table keeps one clock.
func setbEarlier(t *testing.T, resp string, r int, col, age string) []fnsCell {
	t.Helper()
	return []fnsCell{{r, col, strconv.FormatInt(fnsInt(t, fnsCellOf(t, resp, r, col))-1000, 10)},
		{r, age, strconv.FormatInt(fnsInt(t, fnsCellOf(t, resp, r, age))+1, 10)}}
}

// setbMoveTo are the cells of row r of a recorded table that move the time in col to the second sec and its age (in
// age) by as much the other way, so that the table keeps one clock.
func setbMoveTo(t *testing.T, resp string, r int, col, age string, sec int64) []fnsCell {
	t.Helper()
	at := fnsInt(t, fnsCellOf(t, resp, r, col))
	return []fnsCell{{r, col, strconv.FormatInt(sec*1000, 10)},
		{r, age, strconv.FormatInt(fnsInt(t, fnsCellOf(t, resp, r, age))+at/1000-sec, 10)}}
}

// setbAddMax is resp with a `max` of 1 written first in column col's definition, its length fixed.
func setbAddMax(t *testing.T, resp, col string) string {
	t.Helper()
	body := string(httpBody([]byte(resp)))
	at := strings.Index(body, "\n        \""+col+"\":{")
	if at < 0 {
		t.Fatalf("harness: no column %s", col)
	}
	at += len("\n        \"" + col + "\":{")
	return setbWithBody(t, resp, body[:at]+"\"max\":1,"+body[at:])
}

// testDashNormClosersBRelay pins TestNodeInstancesSetB/relay's rows on C's recorded stages.
func testDashNormClosersBRelay(t *testing.T) {
	if got := len(setbRelayRecorded); got != len(setbRelayStages) {
		t.Fatalf("%d recorded stages, want %d", got, len(setbRelayStages))
	}
	st := []string{"nodes", "[1]", "instances", "[0]", "stream"}
	at := func(keys ...string) []string { return append(slices.Clone(st), keys...) }
	local := []string{"nodes", "[0]", "instances", "[0]"}
	for _, stage := range setbRelayStages {
		rec := setbRelayRecorded[stage]
		key := "relay/" + stage
		status, _ := setbRelayState(stage)
		other := map[string]string{"replicating": `"online"`, "online": `"replicating"`}[status]
		cases := []setbNICase{
			// m35 (exactly one replicating chart reads online) is the noquery stage's; any other stage the same text
			{"the stream's other status", at("status"), other, false, ".stream.status"},
			{"one hop", at("hops"), "1", false, ".stream.hops"},
			{"the connection's count 2", at("id"), "2", false, ".stream.id"},
			{"the instances one more", at("replication", "instances"), "9", false, ".replication.instances"},
			{"the parent's handshake another", at("destination", "parents", "[0]", "last_handshake"),
				`"CONNECTED"`, false, ".last_handshake"},
			{"the stream path's start the sender's", at("destination", "streaming_path", "[0]", "since"),
				strconv.FormatInt(rec.sides[1].connected[1], 10), false, ".streaming_path[0].since"},
			{"localhost streams", append(slices.Clone(local), "dyncfg"), `{"status":"online"},"stream":{}`, false,
				"$.nodes[0].instances[0]"},
			{"the oracle online at another stage", at("status"), other, true, "oracle:"},
			{"the oracle's localhost streams", append(slices.Clone(local), "dyncfg"),
				`{"status":"online"},"stream":{}`, true, "oracle:"},
		}
		switch stage {
		case "partial", "noquery":
			// m44 (the stamps swapped) and m43 (an answer without a query zeroes the latest end) read 0; a stamp of
			// another second than the walk's clock
			cases = append(cases,
				setbNICase{"the completion 0 (m43, m44)", at("replication", "completion"), "0", false, ".completion"},
				setbNICase{"the completion 100", at("replication", "completion"), "100", false, ".completion"},
				setbNICase{"the completion a second later", at("replication", "completion"),
					rec.sides[1].completion(rec.ni.flight[1][0] + 1), false, ".completion"})
		default:
			cases = append(cases, setbNICase{"the completion 0", at("replication", "completion"), "0", false,
				".completion"})
		}
		setbCheckNI(t, key, setbRelayRow(stage, rec.port, rec.base), setbRelayFamily(rec.sides), rec.ni, cases)
		var ports [2]string
		for i := range rec.sides {
			ports[i] = rec.sides[i].listen
		}
		child := setbRow(fnsDoc(t, rec.fn.raw[0]), proxiedHost.Hostname)
		fcases := []setbFnCase{
			{"the stream's other status (m35)", []fnsCell{{child, "OutStatus", other}}, false, ".data[1]"},
			{"one hop out", []fnsCell{{child, "OutHops", "1"}}, false, ".data[1]"},
			{"the oracle's stream offline", []fnsCell{{child, "OutStatus", `"offline"`}}, true, "oracle: guard"},
			{"the sender's local port the parent's", []fnsCell{{child, "OutLocalPort", rec.port}}, false,
				"candidate: row 1: OutLocalPort"},
			{"the sender's local port the agent's own", []fnsCell{{child, "OutLocalPort", rec.sides[1].listen}}, false,
				"candidate: row 1: OutLocalPort"},
			{"the child's InSince before its connection", setbMoveTo(t, rec.fn.raw[1], child, "InSince", "InAge",
				rec.sides[1].opened[0]-1), false, "candidate: row 1: InSince"},
			{"the sender's OutSince before its connection", setbMoveTo(t, rec.fn.raw[1], child, "OutSince", "OutAge",
				rec.sides[1].connected[0]-1), false, "candidate: row 1: OutSince"},
			{"the attempt's age a second off", []fnsCell{{child, "OutAttemptAge", strconv.FormatInt(fnsInt(t,
				fnsCellOf(t, rec.fn.raw[1], child, "OutAttemptAge"))+1, 10)}}, false, "candidate: row 1: OutAttemptSince"},
			{"the attempt's max another", []fnsCell{{-1, "OutAttemptSince", "1"}}, false, "columns.OutAttemptSince.max"},
		}
		if slices.Contains(setbRelayVolatile(stage), "OutReplCompletion") {
			fcases = append(fcases,
				setbFnCase{"the completion 0 (m43, m44)", []fnsCell{{child, "OutReplCompletion", "0"}}, false,
					"candidate: row 1: OutReplCompletion"},
				setbFnCase{"the oracle's completion 100", []fnsCell{{child, "OutReplCompletion", "100"}}, true,
					"oracle: guard"},
				setbFnCase{"the completion's max 1", []fnsCell{{-1, "OutReplCompletion", "1"}}, false,
					"columns.OutReplCompletion.max"})
		} else {
			fcases = append(fcases,
				setbFnCase{"the completion 0", []fnsCell{{child, "OutReplCompletion", "0"}}, false, ".data[1]"},
				setbFnCase{"the oracle's completion 0", []fnsCell{{child, "OutReplCompletion", "0"}}, true,
					"oracle: guard"})
		}
		setbCheckFn(t, key, setbRelayAsk(stage, rec.port, rec.sides), rec.fn, ports, rec.launches, fcases)
		// OutLocalPort with a max, which C passes none of
		raw := [2]string{rec.fn.raw[0], setbAddMax(t, rec.fn.raw[1], "OutLocalPort")}
		if p := setbJudgeFn(t, setbRelayAsk(stage, rec.port, rec.sides), rec.fn, ports, rec.launches, raw); !slices.ContainsFunc(p,
			func(s string) bool { return strings.Contains(s, "OutLocalPort has a max") }) {
			t.Errorf("%s, OutLocalPort with a max: problems %q", key, p)
		}
	}
}

// testDashNormClosersBVnode pins TestNodeInstancesSetB/vnode's rows on C's recorded answers.
func testDashNormClosersBVnode(t *testing.T) {
	rec := setbVnodeRecorded
	vnode := []string{"nodes", "[1]", "instances", "[0]"}
	local := []string{"nodes", "[0]", "instances", "[0]"}
	at := func(inst []string, keys ...string) []string { return append(slices.Clone(inst), keys...) }
	req := v2Req{name: "v3-ni", target: "/api/v3/node_instances", status: "200", guard: setbVnodeFacts()}
	setbCheckNI(t, "vnode", req, setbVnodeFamily(rec.sides, rec.firsts), rec.ni, []setbNICase{
		{"the vnode a child", at(vnode, "ingest", "type"), `"child"`, false, ".ingest.type"},
		{"the vnode at hops 0", at(vnode, "ingest", "hops"), "0", false, ".ingest.hops"},
		{"the vnode's start after the agent's", at(vnode, "ingest", "since"),
			strconv.FormatInt(rec.sides[1].started[1]+10, 10), false, ".ingest.since"},
		{"the vnode's first point its first collection", at(vnode, "db", "first_time"),
			strconv.FormatInt(rec.firsts[1][0]-1, 10), false, ".db.first_time"},
		{"localhost's first point the vnode's", at(local, "db", "first_time"),
			strconv.FormatInt(rec.firsts[1][0], 10), false, ".db.first_time"},
		{"the vnode with dyncfg", at(vnode, "dyncfg"), `{"status":"online"}`, false, ".dyncfg.status"},
		{"the vnode stale", at(vnode, "db", "liveness"), `"stale"`, false, ".db.liveness"},
		{"the oracle's vnode a child", at(vnode, "ingest", "type"), `"child"`, true, "oracle:"},
		{"the oracle's localhost initializing", at(local, "ingest", "status"), `"initializing"`, true, "oracle:"},
	})
	ask := setbVnodeAsk(rec.firsts, rec.sides)
	v := setbRow(fnsDoc(t, rec.fn.raw[0]), vnodeName)
	l := setbRow(fnsDoc(t, rec.fn.raw[0]), parentIdentity.Hostname)
	setbCheckFn(t, "vnode", ask, rec.fn, setbPortsOf(rec.sides), rec.launches, []setbFnCase{
		{"the vnode's reason CONNECTED", []fnsCell{{v, "InReason", `"CONNECTED"`}}, false, ".data[1]"},
		{"the vnode's local address empty", []fnsCell{{v, "InLocalIP", `""`}}, false, ".data[1]"},
		{"the vnode at one hop out", []fnsCell{{v, "OutHops", "1"}}, false, ".data[1]"},
		{"the oracle's vnode CONNECTED", []fnsCell{{v, "InReason", `"CONNECTED"`}}, true, "oracle: guard"},
		{"the vnode's first point a second early", []fnsCell{{v, "dbFrom",
			strconv.FormatInt((rec.firsts[1][0]-1)*1000, 10)}, {v, "dbDuration", strconv.FormatInt(fnsInt(t,
			fnsCellOf(t, rec.fn.raw[1], v, "dbDuration"))+1, 10)}}, false, "candidate: row 1 (parity-vnode): dbFrom"},
		{"localhost's first point the vnode's", []fnsCell{{l, "dbFrom", strconv.FormatInt(rec.firsts[1][0]*1000, 10)},
			{l, "dbDuration", strconv.FormatInt(fnsInt(t, fnsCellOf(t, rec.fn.raw[1], l, "dbDuration"))+
				rec.firsts[1][1]-rec.firsts[1][0], 10)}}, false, "candidate: row 0 (parity-parent): dbFrom"},
		{"the vnode's InSince a second early", setbEarlier(t, rec.fn.raw[1], v, "InSince", "InAge"), false,
			"candidate: row 1 (parity-vnode): InSince"},
	})
	// setbLocalQueryable's poll: C's answers read localhost's db online with a second of retention (the younger side's
	// first stored point a second before its walk's clock); anything else is asked again: localhost's db initializing
	// (the younger side's first ask in SA-B's probe p5 read its db and its ingestion initializing; the plant sets
	// db.status alone), its first stored point the walk's second (the table's duration null in SA-B3's probe p1) or 0,
	// a name no node has, localhost without an instance, a body that is no JSON, a cut body
	db := func(raw, status, first string) string {
		if first == "" {
			first = setbNIValue(t, raw, at(local, "db", "first_time"))
		}
		return status + " from " + first + " to " + setbNIValue(t, raw, at(local, "db", "last_time"))
	}
	for i, raw := range rec.ni.raw {
		if got, done := setbLocalPoll([]byte(raw), parentIdentity.Hostname); got != db(raw, `"online"`, "") || !done {
			t.Errorf("setbLocalPoll, C's side %d: %q, %v, want online with a second of retention", i, got, done)
		}
	}
	young := rec.ni.raw[1]
	last := setbNIValue(t, young, at(local, "db", "last_time"))
	body := string(httpBody([]byte(young)))
	for name, c := range map[string]struct {
		answer, host, want string
	}{
		"localhost initializing": {setbPlant(t, young, at(local, "db", "status"), `"initializing"`),
			parentIdentity.Hostname, db(young, `"initializing"`, "")},
		"localhost's first point the walk's second": {setbPlant(t, young, at(local, "db", "first_time"), last),
			parentIdentity.Hostname, db(young, `"online"`, last)},
		"localhost's first point 0": {setbPlant(t, young, at(local, "db", "first_time"), "0"),
			parentIdentity.Hostname, db(young, `"online"`, "0")},
		"another name": {young,
			"parity-other", "no node parity-other"},
		"localhost no instance": {setbPlant(t, young, []string{"nodes", "[0]", "instances"}, "[]"),
			parentIdentity.Hostname, "no instances.[0]"},
		"no JSON": {setbWithBody(t, young, "Unsupported API command"),
			parentIdentity.Hostname, ""},
		"a cut body": {setbWithBody(t, young, body[:len(body)/2]),
			parentIdentity.Hostname, ""},
	} {
		if got, done := setbLocalPoll([]byte(c.answer), c.host); done || (c.want != "" && got != c.want) {
			t.Errorf("setbLocalPoll, %s: %q, %v, want %q asked again", name, got, done, c.want)
		}
	}
}

// testDashNormClosersBFnStream pins fn.stream's node-instances row on C's recorded answers.
func testDashNormClosersBFnStream(t *testing.T) {
	rec := setbFnStreamRecorded
	child := []string{"nodes", "[1]", "instances", "[0]"}
	vn := []string{"nodes", "[2]", "instances", "[0]"}
	at := func(inst []string, keys ...string) []string { return append(slices.Clone(inst), keys...) }
	req := v2Req{name: "v3-ni", target: "/api/v3/node_instances", status: "200", guard: fnStreamNIFacts}
	setbCheckNIDrop(t, "fn.stream", req, fnStreamNIFamily(rec.sides), rec.ni, []setbNICase{
		// the status's dyncfg read for a child (BACKLOG, commit 11: is_localhost() in its place)
		{"the child's dyncfg unavailable", at(child, "dyncfg"), `{"status":"unavailable"}`, false, ".dyncfg.status"},
		{"the child without functions", at(child, "functions"), "{}", false, ".functions"},
		{"the vnode at one hop", at(vn, "ingest", "hops"), "1", false, ".ingest.hops"},
		{"the vnode's dyncfg online", at(vn, "dyncfg"), `{"status":"online"}`, false, ".dyncfg.status"},
		{"the child's start the parent's", at(child, "ingest", "since"),
			strconv.FormatInt(rec.sides[1].started[0], 10), false, ".ingest.since"},
		{"the child's first point before its launch", at(child, "db", "first_time"),
			strconv.FormatInt(rec.sides[1].opened[0]-1, 10), false, ".db.first_time"},
		{"the oracle's child dyncfg unavailable", at(child, "dyncfg"), `{"status":"unavailable"}`, true, "oracle:"},
		{"the oracle's vnode at one hop", at(vn, "ingest", "hops"), "1", true, "oracle:"},
		{"the oracle's child without its gone plugin's method", at(child, "functions", "difftest-gone"), "", true,
			"oracle:"},
		{"the child without its gone plugin's method", at(child, "functions", "difftest-gone"), "", false,
			".functions.<members>"},
	})
}

// testDashNormClosersBArchived pins TestNodeInstancesSetB/archived's rows on C's recorded answers.
func testDashNormClosersBArchived(t *testing.T) {
	rec := setbArchivedRecorded
	inst := []string{"nodes", "[0]", "instances", "[0]"}
	at := func(keys ...string) []string { return append(slices.Clone(inst), keys...) }
	setbCheckNIDrop(t, "archived", setbArchivedRow(), nodeInstancesFamily(rec.sides), rec.ni, []setbNICase{
		{"the since the agent's start", at("ingest", "since"), strconv.FormatInt(rec.sides[1].started[0], 10), false,
			".ingest.since"},
		{"the since the clock", at("ingest", "since"), strconv.FormatInt(rec.ni.flight[1][0], 10), false,
			".ingest.since"},
		{"the host with functions", at("health"), `{"status":"disabled"},"functions":{}`, false,
			".instances[0].<members>"},
		{"offline", at("ingest", "status"), `"offline"`, false, ".ingest.status"},
		{"one connection", at("ingest", "id"), "1", false, ".ingest.id"},
		{"no hop", at("ingest", "hops"), "0", false, ".ingest.hops"},
		{"live", at("db", "liveness"), `"live"`, false, ".db.liveness"},
		{"no ingestion", at("ingest"), "", false, ".instances[0].<members>"},
		{"the oracle's since the agent's start", at("ingest", "since"), strconv.FormatInt(rec.sides[0].started[0], 10),
			true, "oracle:"},
		{"the oracle's since its data's start", at("ingest", "since"),
			setbNIValue(t, rec.ni.raw[0], at("db", "first_time")), true, "oracle:"},
		{"the oracle's host with functions", at("health"), `{"status":"disabled"},"functions":{}`, true, "oracle:"},
		{"the oracle's child offline", at("ingest", "status"), `"offline"`, true, "oracle:"},
	})
	// the oracle's since its data's start and its age as much longer: one clock still (NOW-SINCE), so that only the
	// rule of an archived host's since (setbArchivedSince) refuses it
	since, first := fnsInt(t, setbNIValue(t, rec.ni.raw[0], at("ingest", "since"))),
		fnsInt(t, setbNIValue(t, rec.ni.raw[0], at("db", "first_time")))
	age := fnsInt(t, setbNIValue(t, rec.ni.raw[0], at("ingest", "age")))
	o := setbPlant(t, setbPlant(t, rec.ni.raw[0], at("ingest", "since"), strconv.FormatInt(first, 10)),
		at("ingest", "age"), strconv.FormatInt(age+since-first, 10))
	if p := setbJudgeNI(setbArchivedRow(), nodeInstancesFamily(rec.sides), rec.ni, o, rec.ni.raw[1]); !slices.ContainsFunc(p,
		func(s string) bool {
			return strings.HasPrefix(s, "oracle:") && strings.Contains(s, "want the since the data's end")
		}) {
		t.Errorf("archived, the oracle's since its data's start, its age as long: problems %q", p)
	}
	child := setbRow(fnsDoc(t, rec.fn.raw[0]), childHost.Hostname)
	dbTo := fnsInt(t, fnsCellOf(t, rec.fn.raw[1], child, "dbTo"))
	duration := fnsInt(t, fnsCellOf(t, rec.fn.raw[1], child, "dbDuration"))
	setbCheckFn(t, "archived", setbArchivedAsk, rec.fn, setbPortsOf(rec.sides), rec.launches, []setbFnCase{
		{"the child online", []fnsCell{{child, "InStatus", `"online"`}}, false, ".data[1]"},
		{"the child of severity normal", []fnsCell{{child, "rowOptions", `{"severity":"normal"}`}}, false, ".data[1]"},
		{"the child's reason another", []fnsCell{{child, "InReason", `"DISCONNECTED SOCKET CLOSED BY REMOTE END"`}},
			false, ".data[1]"},
		{"the child's InSince the agent's start", setbMoveTo(t, rec.fn.raw[1], child, "InSince", "InAge",
			rec.sides[1].started[0]), false, "candidate: row 1: InSince"},
		{"the child's retention a second longer", []fnsCell{{child, "dbTo", strconv.FormatInt(dbTo+1000, 10)},
			{child, "dbDuration", strconv.FormatInt(duration+1, 10)}}, false, ".data[1]"},
		{"the oracle's child online", []fnsCell{{child, "InStatus", `"online"`}}, true, "oracle: guard"},
		{"the oracle's child of severity normal", []fnsCell{{child, "rowOptions", `{"severity":"normal"}`}}, true,
			"oracle: guard"},
	})
}

// setbNIValue is the text of the value at path in a recorded node-instance answer.
func setbNIValue(t *testing.T, raw string, path []string) string {
	t.Helper()
	v, err := ParseJSON(httpBody([]byte(raw)))
	if err != nil {
		t.Fatalf("harness: %v", err)
	}
	x, err := dashAt(v, path...)
	if err != nil {
		t.Fatalf("harness: %v", err)
	}
	return x.String()
}

// testDashNormClosersBClaim pins TestNodeInstancesSetB/claim's rows on C's recorded stages.
func testDashNormClosersBClaim(t *testing.T) {
	if got := len(setbClaimRecorded); got != len(setbClaimStages) {
		t.Fatalf("%d recorded stages, want %d", got, len(setbClaimStages))
	}
	inst := []string{"nodes", "[0]", "instances", "[0]"}
	at := func(keys ...string) []string { return append(slices.Clone(inst), keys...) }
	for _, stage := range setbClaimStages {
		rec := setbClaimRecorded[stage]
		key := "claim/" + stage
		later := strconv.FormatInt(rec.sides[1].child[1]-4+1, 10)
		cases := []setbNICase{
			{"the collected counts 0", at("ingest", "metrics"), "0", false, ".ingest.metrics"},
			{"no connection", at("ingest", "id"), "0", false, ".ingest.id"},
			{"the first point a second later", at("db", "first_time"), later, false, ".db.first_time"},
			{"the oracle's first point a second later", at("db", "first_time"),
				strconv.FormatInt(rec.sides[0].child[1]-4+1, 10), true, "oracle:"},
		}
		if stage == "claimed" {
			cases = append(cases,
				setbNICase{"the vnode a child", at("ingest", "type"), `"child"`, false, ".ingest.type"},
				setbNICase{"the since the child's connection", at("ingest", "since"),
					strconv.FormatInt(rec.sides[1].child[0], 10), false, ".ingest.since"},
				setbNICase{"the vnode stale", at("db", "liveness"), `"stale"`, false, ".db.liveness"},
				setbNICase{"the oracle's vnode a child", at("ingest", "type"), `"child"`, true, "oracle:"},
				setbNICase{"the oracle's vnode stale", at("db", "liveness"), `"stale"`, true, "oracle:"})
		} else {
			cases = append(cases,
				setbNICase{"the vnode still virtual", at("ingest", "type"), `"virtual"`, false, ".ingest.type"},
				setbNICase{"online", at("ingest", "status"), `"online"`, false, ".ingest.status"},
				setbNICase{"the since the agent's start", at("ingest", "since"),
					strconv.FormatInt(rec.sides[1].started[0], 10), false, ".ingest.since"},
				setbNICase{"the vnode live", at("db", "liveness"), `"live"`, false, ".db.liveness"},
				setbNICase{"the stored end the clock", at("db", "last_time"), strconv.FormatInt(rec.ni.flight[1][0], 10),
					false, ".db.last_time"},
				setbNICase{"the oracle's vnode online", at("ingest", "status"), `"online"`, true, "oracle:"},
				setbNICase{"the oracle's stored end the clock", at("db", "last_time"),
					strconv.FormatInt(rec.ni.flight[0][0], 10), true, "oracle:"},
				setbNICase{"the oracle's since the agent's start", at("ingest", "since"),
					strconv.FormatInt(rec.sides[0].started[0], 10), true, "oracle:"})
		}
		setbCheckNI(t, key, setbClaimRow(stage), setbClaimFamily(rec.sides), rec.ni, cases)
		v := setbRow(fnsDoc(t, rec.fn.raw[0]), vnodeName)
		from := fnsInt(t, fnsCellOf(t, rec.fn.raw[1], v, "dbFrom"))
		duration := fnsInt(t, fnsCellOf(t, rec.fn.raw[1], v, "dbDuration"))
		dbLater := []fnsCell{{v, "dbFrom", strconv.FormatInt(from+1000, 10)},
			{v, "dbDuration", strconv.FormatInt(duration-1, 10)}}
		fcases := []setbFnCase{
			{"the vnode's dbFrom a second later", dbLater, false, "candidate: row 1: dbFrom"},
			{"no connection", []fnsCell{{v, "InConnections", "0"}}, false, ".data[1]"},
		}
		if stage == "claimed" {
			fcases = append(fcases,
				setbFnCase{"the vnode's reason CONNECTED", []fnsCell{{v, "InReason", `"CONNECTED"`}}, false, ".data[1]"},
				setbFnCase{"the vnode's local address empty", []fnsCell{{v, "InLocalIP", `""`}}, false, ".data[1]"},
				setbFnCase{"the vnode offline", []fnsCell{{v, "InStatus", `"offline"`}}, false, ".data[1]"},
				setbFnCase{"the vnode's InSince the child's connection", setbMoveTo(t, rec.fn.raw[1], v, "InSince",
					"InAge", rec.sides[1].child[0]), false, "candidate: row 1: InSince"},
				setbFnCase{"the oracle's vnode offline", []fnsCell{{v, "InStatus", `"offline"`}}, true, "oracle: guard"},
				setbFnCase{"the oracle's vnode's local address empty", []fnsCell{{v, "InLocalIP", `""`}}, true,
					"oracle: guard"})
		} else {
			fcases = append(fcases,
				setbFnCase{"the vnode's reason VIRTUAL NODE", []fnsCell{{v, "InReason", `"VIRTUAL NODE"`}}, false,
					".data[1]"},
				setbFnCase{"the vnode of severity normal", []fnsCell{{v, "rowOptions", `{"severity":"normal"}`}}, false,
					".data[1]"},
				setbFnCase{"the vnode's local address localhost", []fnsCell{{v, "InLocalIP", `"localhost"`}}, false,
					".data[1]"},
				setbFnCase{"the vnode's InSince the agent's start", setbMoveTo(t, rec.fn.raw[1], v, "InSince", "InAge",
					rec.sides[1].started[0]), false, "candidate: row 1 (parity-vnode): InSince"},
				setbFnCase{"the oracle's vnode online", []fnsCell{{v, "InStatus", `"online"`}}, true, "oracle: guard"},
				setbFnCase{"the oracle's reason VIRTUAL NODE", []fnsCell{{v, "InReason", `"VIRTUAL NODE"`}}, true,
					"oracle: guard"},
				setbFnCase{"the oracle's vnode of severity normal", []fnsCell{{v, "rowOptions", `{"severity":"normal"}`}},
					true, "oracle: guard"})
		}
		var ports [2]string
		for i := range rec.sides {
			ports[i] = rec.sides[i].listen
		}
		setbCheckFn(t, key, setbClaimAsk(stage, rec.sides), rec.fn, ports, rec.launches, fcases)
	}
}

// testDashNormClosersBRound pins the tables' comparison as it runs (fnStreamingRound with set B's asks), asked of two
// stub agents writing C's recorded tables at the second they answer (setbStubPair): C's pair holds; a table with one
// thing wrong is reported, on the oracle as the failure that ends the test. The stages whose cells do not follow the
// clock: replicating's partial (its completion is the answer's), relay's stuck and online (100), the vnode's, the
// archived host's and the claim's (an offline host's retention and since stay; only the ages move).
func testDashNormClosersBRound(t *testing.T) {
	type round struct {
		name     string
		ask      fnStreamingAsk
		raw      [2]string
		launches [2]int64
		row      string
	}
	partial, stuck, online := setbReplRecorded["partial"], setbRelayRecorded["stuck"], setbRelayRecorded["online"]
	rounds := []round{
		{"replicating/partial", setbReplAsk("partial", partial.sides), partial.fn.raw, partial.launches,
			setbReplHost.Hostname},
		{"relay/stuck", setbRelayAsk("stuck", stuck.port, stuck.sides), stuck.fn.raw, stuck.launches,
			proxiedHost.Hostname},
		{"relay/online", setbRelayAsk("online", online.port, online.sides), online.fn.raw, online.launches,
			proxiedHost.Hostname},
		{"vnode", setbVnodeAsk(setbVnodeRecorded.firsts, setbVnodeRecorded.sides), setbVnodeRecorded.fn.raw,
			setbVnodeRecorded.launches, vnodeName},
		{"archived", setbArchivedAsk, setbArchivedRecorded.fn.raw, setbArchivedRecorded.launches, childHost.Hostname},
	}
	for _, stage := range setbClaimStages {
		rec := setbClaimRecorded[stage]
		rounds = append(rounds, round{"claim/" + stage, setbClaimAsk(stage, rec.sides), rec.fn.raw, rec.launches,
			vnodeName})
	}
	for _, x := range rounds {
		if r := fnStreamingRound(setbStubPair(t, x.raw, x.launches), x.ask); r.fatal != "" || len(r.problems) > 0 {
			t.Errorf("%s: C's tables: fatal %q, problems %q", x.name, r.fatal, r.problems)
		}
		row := setbRow(fnsDoc(t, x.raw[0]), x.row)
		bad := fnsWith(t, x.raw[1], fnsCell{row, "InHops", "9"})
		if r := fnStreamingRound(setbStubPair(t, [2]string{x.raw[0], bad}, x.launches), x.ask); r.fatal != "" ||
			!slices.ContainsFunc(r.problems, func(s string) bool { return strings.Contains(s, "data[1]") }) {
			t.Errorf("%s: the candidate's InHops 9: fatal %q, problems %q", x.name, r.fatal, r.problems)
		}
		if r := fnStreamingRound(setbStubPair(t, [2]string{fnsWith(t, x.raw[0], fnsCell{row, "InHops", "9"}), x.raw[1]},
			x.launches), x.ask); !strings.Contains(r.fatal, "oracle:") {
			t.Errorf("%s: the oracle's InHops 9: fatal %q, problems %q", x.name, r.fatal, r.problems)
		}
	}
	// the asks' own parts reach the round: the partial's completion masked and held, the relay's attempt held
	if r := fnStreamingRound(setbStubPair(t, [2]string{partial.fn.raw[0], fnsWith(t, partial.fn.raw[1],
		fnsCell{1, "InReplCompletion", "100"})}, partial.launches), setbReplAsk("partial", partial.sides)); r.fatal != "" ||
		!slices.ContainsFunc(r.problems, func(s string) bool { return strings.Contains(s, "InReplCompletion is 100") }) {
		t.Errorf("replicating/partial: the candidate's completion 100: fatal %q, problems %q", r.fatal, r.problems)
	}
	if r := fnStreamingRound(setbStubPair(t, [2]string{online.fn.raw[0], fnsWith(t, online.fn.raw[1],
		fnsCell{1, "OutLocalPort", online.port})}, online.launches), setbRelayAsk("online", online.port, online.sides)); r.fatal != "" ||
		!slices.ContainsFunc(r.problems, func(s string) bool { return strings.Contains(s, "OutLocalPort") }) {
		t.Errorf("relay/online: the candidate's local port the parent's: fatal %q, problems %q", r.fatal, r.problems)
	}
}

// setbDateLater is raw with its head's Date a second later (the expiry is read against it, fnStreamingDoc).
func setbDateLater(t *testing.T, raw string) string {
	t.Helper()
	head, body, _ := strings.Cut(raw, "\r\n\r\n")
	lines := strings.Split(head, "\r\n")
	for k, l := range lines {
		if v, ok := strings.CutPrefix(l, "Date: "); ok {
			d, err := http.ParseTime(v)
			if err != nil {
				t.Fatalf("harness: %v", err)
			}
			lines[k] = "Date: " + d.Add(time.Second).UTC().Format(http.TimeFormat)
		}
	}
	return strings.Join(lines, "\r\n") + "\r\n\r\n" + body
}

// testDashNormClosersBVariation pins what each side may differ in that C does: a partial answer taken a second later
// on the candidate (its own window and completion: PARTIAL on both, the table's cell masked and held), the relay's
// table read a second later (its clock, ages and completion one second on: masked and held); and the vnode's first
// stored points read from the fake plugin's own records (setbVnodeFirsts).
func testDashNormClosersBVariation(t *testing.T) {
	partial := setbReplRecorded["partial"]
	later := partial.sides
	later[1].answered = [2]int64{partial.sides[1].answered[1] + 1, partial.sides[1].answered[1] + 1}
	text := later[1].partials()[0]
	ingest := []string{"nodes", "[0]", "instances", "[0]", "ingest", "replication", "completion"}
	cand := setbPlant(t, partial.ni.raw[1], ingest, text)
	if p := setbJudgeNI(setbReplRow("partial", partial.base), setbReplFamily(later), partial.ni, partial.ni.raw[0],
		cand); len(p) > 0 {
		t.Errorf("replicating/partial: the candidate's answer a second later: %q", p)
	}
	child := setbRow(fnsDoc(t, partial.fn.raw[0]), setbReplHost.Hostname)
	raw := [2]string{partial.fn.raw[0], fnsWith(t, partial.fn.raw[1], fnsCell{child, "InReplCompletion", text})}
	if p := setbJudgeFn(t, setbReplAsk("partial", later), partial.fn, setbPortsOf([2]niSide{later[0].niSide,
		later[1].niSide}), partial.launches, raw); len(p) > 0 {
		t.Errorf("replicating/partial: the candidate's table a second later: %q", p)
	}
	for _, stage := range []string{"partial", "noquery"} {
		rec := setbRelayRecorded[stage]
		resp := rec.fn.raw[1]
		row := setbRow(fnsDoc(t, resp), proxiedHost.Hostname)
		clock := fnsClockOf(t, resp)
		moved := setbDateLater(t, setbWithBody(t, resp, clock.at(clock.clock+1)))
		moved = fnsWith(t, moved, fnsCell{row, "OutReplCompletion", rec.sides[1].completion(clock.clock + 1)})
		rec.fn.flight[1] = [2]int64{rec.fn.flight[1][0], rec.fn.flight[1][1] + 1}
		if p := setbJudgeFn(t, setbRelayAsk(stage, rec.port, rec.sides), rec.fn, [2]string{rec.sides[0].listen,
			rec.sides[1].listen}, rec.launches, [2]string{rec.fn.raw[0], moved}); len(p) > 0 {
			t.Errorf("relay/%s: the candidate's table a second later: %q", stage, p)
		}
	}
	// the fake plugin's records: three blocks of the vnode's chart, two of localhost's
	var ls [2]plugin.Layout
	for i := range ls {
		ls[i] = plugin.Layout{Dir: t.TempDir()}
		var b strings.Builder
		for k := int64(0); k < 5; k++ {
			b.WriteString(`{"t":"2026-10-10T00:00:00Z","kind":"step"}` + "\n")
			b.WriteString(`{"t":"2026-10-10T00:00:00Z","kind":"collected","sec":` + strconv.FormatInt(1000+10*int64(i)+k, 10) +
				"}\n")
		}
		if err := os.WriteFile(filepath.Join(ls[i].Dir, "start-000.jsonl"), []byte(b.String()), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if got, want := setbVnodeFirsts(t, ls), [2][2]int64{{1001, 1004}, {1011, 1014}}; got != want {
		t.Errorf("setbVnodeFirsts = %v, want %v", got, want)
	}
}

// testDashNormClosersB is set B's pins (TestDashNormClosers' `b`).
func testDashNormClosersB(t *testing.T) {
	t.Run("format", testDashNormClosersBFormat)
	t.Run("renders", testDashNormClosersBRenders)
	t.Run("replicating", testDashNormClosersBRepl)
	t.Run("relay", testDashNormClosersBRelay)
	t.Run("vnode", testDashNormClosersBVnode)
	t.Run("archived", testDashNormClosersBArchived)
	t.Run("claim", testDashNormClosersBClaim)
	t.Run("fn-stream", testDashNormClosersBFnStream)
	t.Run("round", testDashNormClosersBRound)
	t.Run("variation", testDashNormClosersBVariation)
}
