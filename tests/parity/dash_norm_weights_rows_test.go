// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"slices"
	"strings"
	"testing"
)

// testDashNormWeightsRows pins the weights rows of H38 (weights_rows_test.go) on the answers of one C-against-C run
// (dashNormWeightsRows, dashNormWeightsInterrupted): every recorded row is a row of the checks and back; each row's
// recorded pair is judged as v2Round judges a live one (dashNormWeightsJudge) and shows no difference, with the rows
// of a check in its order so that the facts that record a row's numbers and those that read them see what the check
// sees; each guard refuses the named wrong answers of dashNormWeightsPlanted, each planted where its row's guard reads;
// the relative render and the 499's judge are pinned on their recorded inputs and on planted ones.
func testDashNormWeightsRows(t *testing.T) {
	checks := []struct {
		name string
		rows []weightsRow
	}{
		{"weights", weightsAPIRows()},
		{"weightsv1", weightsV1Rows()},
		{"hosts", slices.Concat(weightsHostsRows(), weightsGoneRows(), weightsOneCPURows())},
		{"tiers", weightsTiersRows()},
		{"big", weightsBigRows()},
	}
	var keys []string
	for _, c := range checks {
		for _, r := range c.rows {
			keys = append(keys, c.name+"/"+r.req.name)
		}
	}
	slices.Sort(keys)
	if got := slices.Sorted(maps.Keys(dashNormWeightsRows)); !slices.Equal(got, keys) {
		t.Fatalf("the recorded rows are %q, the checks' rows %q", got, keys)
	}
	planted := 0
	for _, c := range checks {
		for _, r := range c.rows {
			key := c.name + "/" + r.req.name
			rec := dashNormWeightsRows[key]
			if problems := dashNormWeightsJudge(r.req, r.fam, rec); len(problems) > 0 {
				t.Errorf("%s: the recorded pair: %q", key, problems)
			}
			for _, p := range dashNormWeightsPlanted[key] {
				planted++
				if err := dashNormWeightsPlant(r.req, r.fam, rec, p); err == nil {
					t.Errorf("%s: its guard takes %s", key, p.name)
				} else if strings.HasPrefix(err.Error(), "harness:") {
					t.Errorf("%s: %s: %v", key, p.name, err)
				}
			}
		}
	}
	for key := range dashNormWeightsPlanted {
		if _, ok := dashNormWeightsRows[key]; !ok {
			t.Errorf("planted answers for %s, which is no recorded row", key)
		}
	}
	if planted < 60 {
		t.Errorf("only %d planted answers were judged", planted)
	}
	dashNormWeightsRelative(t)
	dashNormWeightsInterruptedPins(t)
}

// dashNormWeightsJudge judges a recorded pair as v2Round judges a live one (v2Judge), the network aside. The problems,
// none when they agree.
func dashNormWeightsJudge(req v2Req, fam v2Family, rec dashNormWeightsRow) []string {
	var a v2Answer
	for i := range a.raw {
		a.raw[i] = []byte(rec.heads[i] + "\r\n\r\n" + rec.bodies[i])
	}
	a.flight = rec.flight
	v2Judge(&a, req, fam, [2]string{string(Oracle), string(Candidate)}, [2]string{})
	return a.problems
}

// dashNormWeightsWrong is a named wrong answer of a row: the oracle's answer with the value at path set to the JSON
// text value, or, for a row that answers no 200, its body replaced by value (path empty).
type dashNormWeightsWrong struct {
	name, value string
	path        []string
}

// dashNormWeightsSet sets the value at path (dashAt's steps) of v to the JSON text want, in place.
func dashNormWeightsSet(v *Value, want string, path ...string) error {
	w, err := ParseJSON([]byte(want))
	if err != nil {
		return fmt.Errorf("harness: %s: %v", want, err)
	}
	cur := v
	for _, step := range path {
		if n, ok := dashIndex(step); ok {
			if cur.Kind != KindArray || n < 0 || n >= len(cur.Items) {
				return fmt.Errorf("harness: no %s", strings.Join(path, "."))
			}
			cur = &cur.Items[n]
			continue
		}
		found := false
		for i := range cur.Members {
			if cur.Members[i].Key == step {
				cur, found = &cur.Members[i].Value, true
				break
			}
		}
		if !found {
			return fmt.Errorf("harness: no %s", strings.Join(path, "."))
		}
	}
	*cur = w
	return nil
}

// dashNormWeightsPlant hands the row's guard its oracle's answer, normalised by fam, with the wrong answer planted:
// the guard's error, nil when it takes the answer, or a `harness:` error when the answer cannot be planted.
func dashNormWeightsPlant(req v2Req, fam v2Family, rec dashNormWeightsRow, wrong dashNormWeightsWrong) error {
	if req.status != "200" {
		if len(wrong.path) > 0 {
			return fmt.Errorf("harness: a path planted in a %s", req.status)
		}
		return req.guard(Value{Kind: KindString, Text: wrong.value})
	}
	body := fam.normalise(0, fam.v2Clock(req.target, rec.flight[0]), rec.flight[0], []byte(rec.bodies[0]))
	doc, err := ParseJSON(body)
	if err != nil {
		return fmt.Errorf("harness: %v", err)
	}
	if len(wrong.path) == 0 {
		return fmt.Errorf("harness: no path planted in a 200")
	}
	if err := dashNormWeightsSet(&doc, wrong.value, wrong.path...); err != nil {
		return err
	}
	return req.guard(doc)
}

// dashNormWeightsPlanted are, per recorded row, wrong answers its guard refuses: the port's plausible mistakes (the
// planted bugs of the weights' teeth, m12 to m42, where a row is their judge) and C's answers to a neighbour's request.
var dashNormWeightsPlanted = map[string][]dashNormWeightsWrong{
	"weights/ks2-shift": {
		{"the baseline's start not rewritten", "1700000080", []string{"request", "baseline", "baseline_after"}},
		{"the baseline asked without the shifts", "500", []string{"view", "baseline", "points"}},
		{"level at KSfbar's end", "1", []string{"result", "[1]", "[5]"}},
		{"level of the swapped series (m12)", "0.9", []string{"result", "[1]", "[5]"}},
	},
	"weights/ks2-raw": {
		{"level ranked apart, as without raw", "0.5", []string{"result", "[1]", "[5]"}},
		{"a shift", "2000", []string{"view", "baseline", "points"}},
	},
	"weights/shift-round": {
		{"2.5 truncated to 2: one shift", "1699999940", []string{"request", "baseline", "baseline_after"}},
		{"one shift's points", "1000", []string{"view", "baseline", "points"}},
		{"level at KSfbar's end", "1", []string{"result", "[1]", "[5]"}},
	},
	"weights/one-sided": {
		{"the baseline's end taken as absolute (a two-sided test)", "-94608001",
			[]string{"request", "baseline", "baseline_before"}},
		{"five shifts kept", "16000", []string{"view", "baseline", "points"}},
		{"the contexts walked", "6", []string{"total_dimensions_count"}},
	},
	"weights/relative": {
		{"the baseline not stretched", `"END-1099"`, []string{"request", "baseline", "baseline_after"}},
		{"the highlight a second longer", `"END-1000"`, []string{"request", "window", "after"}},
		{"no metric examined", "0", []string{"total_dimensions_count"}},
	},
	"weights/bad-baseline": {
		{"the points' refusal", `{"error": "Too few points available, at least 15 are needed." }`, nil},
	},
	"weights/few-points":        {{"the baseline's refusal", `{"error": "Invalid baseline time-range." }`, nil}},
	"weights/few-points-volume": {{"the window's refusal", `{"error": "Invalid selected time-range." }`, nil}},
	"weights/few-points-baseline": {
		{"the points refused first", `{"error": "Too few points available, at least 15 are needed." }`, nil},
	},
	"weights/one-query": {
		{"the per-tier points added", "[726]", []string{"db", "db_points_per_tier"}},
	},
	"weights/walk": {
		{"no per-tier points", "[0]", []string{"db", "db_points_per_tier"}},
		{"another value of level", "29", []string{"result", "[1]", "[5]"}},
	},
	"weights/echo-rfc3339": {
		{"the window's start as seconds", "1700000120", []string{"request", "window", "after"}},
		{"the options in another order", `["null2zero","unaligned","rfc3339","minify"]`, []string{"request", "options"}},
	},
	"weights/echo-tier": {
		{"no tier selected", "null", []string{"request", "window", "tier"}},
		{"nonzero kept", `["nonzero","null2zero","unaligned","selected-tier"]`, []string{"request", "options"}},
	},
	"weights/echo-trimmed": {
		{"the grouping's name as asked", `"trimmed-mean"`, []string{"view", "time_group"}},
		{"the request's grouping as asked", `"trimmed-mean"`, []string{"request", "aggregations", "time", "time_group"}},
		{"the mean, not the trimmed mean", "29.8429752", []string{"result", "[1]", "[5]"}},
	},
	"weights/mc-v2": {{"version 1's answer", `{"error": "no results produced." }`, nil}},
	"weightsv1/mc-none": {
		{"the error without its spaces", `{"error":"no results produced."}`, nil},
	},
	"weightsv1/subpath": {
		{"another command's text", `API command 'metric_correlations' does not support subpaths.`, nil},
	},
	"weightsv1/v1-echo": {
		{"the grouping's name as asked", `"trimmed-mean"`, []string{"group"}},
		{"the window's start as seconds", "1700000120", []string{"after"}},
		{"the mean, not the trimmed mean", "29.8429752",
			[]string{"contexts", "fixture.weights", "charts", "fixture.weights", "dimensions", "level"}},
	},
	"weightsv1/limit-hierarchy": {
		{"the chart weighed by its selected dimension", "100",
			[]string{"contexts", "fixture.limith", "charts", "fixture.limith-a", "weight"}},
		{"the context weighed by its selected dimension", "100", []string{"contexts", "fixture.limith", "weight"}},
		{"the unselected chart printed", `{"fixture.limith-a":{"dimensions":{"d0001":100},"weight":50.5},` +
			`"fixture.limith-b":{"dimensions":{},"weight":46}}`, []string{"contexts", "fixture.limith", "charts"}},
	},
	"hosts/hash-c1":  {{"no version", "0", []string{"versions", "contexts_hard_hash"}}},
	"hosts/hash-all": {{"the single sum", "29", []string{"versions", "contexts_hard_hash"}}},
	"hosts/hash-one": {{"the sum twice", "58", []string{"versions", "contexts_hard_hash"}}},
	"hosts/hash-two": {
		{"the scope's sum twice", "58", []string{"versions", "contexts_hard_hash"}},
		{"the single sum", "29", []string{"versions", "contexts_hard_hash"}},
	},
	"hosts/dup-plain": {
		{"the named instance without its name", `[{"id":"fixture.weights","ii":0},{"id":"fixture.weightsks2","ii":1},` +
			`{"id":"fixture.weightsnm","ii":2},{"id":"fixture.weightsgap","ii":3}]`, []string{"dictionaries", "instances"}},
	},
	"hosts/dup-limit": {
		{"the limit counted over one host", `{"limit":1,"total":9,"returned":1,"unit":"dimensions","truncated":true,` +
			`"summary_scope":"all"}`,
			[]string{"result_limit"}},
		{"the other node not listed", `[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,` +
			`"st":{"ai":0,"code":200,"msg":""}}]`, []string{"dictionaries", "nodes"}},
	},
	"hosts/dup-mc": {{"one object per chart", `{"fixture.weights":{}}`, []string{"correlated_charts"}}},
	"hosts/group-instance": {
		{"the instance without its node", `"fixture.weightsnm"`, []string{"result", "[2]", "id"}},
		{"the instance named by its id", `"fixture.weightsnm@parity-child2"`, []string{"result", "[2]", "nm"}},
	},
	"hosts/group-instance-node": {
		{"the node joined by @", `"fixture.weightsnm@5a1e0000-0000-4000-8000-0000000000cc"`, []string{"result", "[2]", "id"}},
	},
	"hosts/group-dim-units": {{"a dimension by its id", `"n1,units"`, []string{"result", "[4]", "id"}}},
	"hosts/group-node":      {{"a node by its hostname", `"parity-child2"`, []string{"result", "[1]", "id"}}},
	"hosts/hidden-one": {
		{"the hidden column counted as a query", "2", []string{"db", "db_queries"}},
		{"the hidden metric not examined", "2", []string{"total_dimensions_count"}},
		{"full's raw value (percentage skipped)", "7", []string{"result", "[0]", "[5]"}},
	},
	"hosts/hidden-one-raw": {{"the hidden column not exposed with raw", "1", []string{"db", "db_queries"}}},
	"hosts/hidden-walk": {
		{"the hidden metric not queried", "10", []string{"db", "db_queries"}},
		{"the hidden metric exposed", "10", []string{"correlated_dimensions"}},
	},
	"hosts/hidden-walk-raw": {{"the hidden metric not exposed with raw", "9", []string{"correlated_dimensions"}}},
	"hosts/hole-one": {
		{"the stopped metric exposed as a query", "2", []string{"db", "db_queries"}},
		{"the stopped metric not examined", "1", []string{"total_dimensions_count"}},
	},
	"hosts/hole-walk": {{"the stopped metric registered as 0 (m14)", "10", []string{"correlated_dimensions"}}},
	"hosts/later":     {{"the collected contexts gated off", "0", []string{"total_dimensions_count"}}},
	"hosts/later-gone": {
		{"the contexts walked as collected (m19)", "11", []string{"total_dimensions_count"}},
		{"the metrics queried", "11", []string{"db", "db_queries"}},
	},
	"hosts/1cpu-hash-all": {{"the sum twice", "58", []string{"versions", "contexts_hard_hash"}}},
	"hosts/1cpu-hash-two": {{"the first child's sum added", "39", []string{"versions", "contexts_hard_hash"}}},
	"tiers/tier0-one":     {{"per-tier points on the one query", "[484,0]", []string{"db", "db_points_per_tier"}}},
	"tiers/tier1-one": {
		{"tier 0's level (m41)", "29.8429752", []string{"result", "[1]", "[5]"}},
		{"no tier selected", "null", []string{"request", "window", "tier"}},
	},
	"tiers/tier0-walk": {{"tier 1 read", "[0,726]", []string{"db", "db_points_per_tier"}}},
	"tiers/tier1-walk": {{"tier 0 read (m41)", "[96,0]", []string{"db", "db_points_per_tier"}}},
	"tiers/tier-over": {
		{"the tier echoed", "5", []string{"request", "window", "tier"}},
		{"selected-tier kept", `["null2zero","unaligned","selected-tier","raw"]`, []string{"request", "options"}},
	},
	"tiers/tier1-ks2": {{"tier 0 read (m41)", "[168,0]", []string{"db", "db_points_per_tier"}}},
	"big/timeout-wrap": {
		{"the interrupt's answer", `{"error": "interrupted" }`, nil},
		{"the time-range refusal", `{"error": "Invalid selected time-range." }`, nil},
	},
}

// dashNormWeightsRelative pins the relative render: the recorded pair (judged with the rows) shows no difference; a
// candidate whose window is a second off, or whose end is not a second before its flight, is reported.
func dashNormWeightsRelative(t *testing.T) {
	rec := dashNormWeightsRows["weights/relative"]
	var req v2Req
	for _, r := range weightsPreludeRows() {
		if r.req.name == "relative" {
			req = r.req
		}
	}
	if req.name == "" {
		t.Fatalf("no relative row")
	}
	fam := weightsRelativeFamily
	end := dashNormWeightsEnd(t, rec.bodies[1])
	for name, c := range map[string]struct {
		body   string
		flight [2]int64
		at     string
	}{
		"a highlight a second longer": {strings.Replace(rec.bodies[1], fmt.Sprintf(`"after":%d`, end-999),
			fmt.Sprintf(`"after":%d`, end-1000), 1), rec.flight[1], "$.request.window.after"},
		"an end five seconds before its flight": {rec.bodies[1], [2]int64{rec.flight[1][0] + 6, rec.flight[1][1] + 6},
			"$.request.window"},
	} {
		got := dashNormWeightsJudge(req, fam, dashNormWeightsRow{heads: rec.heads, bodies: [2]string{rec.bodies[0],
			c.body}, flight: [2][2]int64{rec.flight[0], c.flight}})
		if !slices.ContainsFunc(got, func(p string) bool { return strings.HasPrefix(p, c.at) }) {
			t.Errorf("weightsRelativeRender: %s gives %q, want a difference at %s", name, got, c.at)
		}
	}
	// the render alone: the ends rewritten from the first `before`, nothing else
	body := []byte(`{"request":{"window":{"after":99001,"before":100000,"points":500},"baseline":{"baseline_after":` +
		`98002,"baseline_before":99001}},"view":{"window":{"after":99001,"before":100000,"duration":999},` +
		`"baseline":{"after":98002,"before":99001,"duration":999}}}`)
	want := `{"request":{"window":{"after":"END-999","before":"END","points":500},"baseline":{"baseline_after":` +
		`"END-1998","baseline_before":"END-999"}},"view":{"window":{"after":"END-999","before":"END","duration":999},` +
		`"baseline":{"after":"END-1998","before":"END-999","duration":999}}}`
	for _, flight := range [][2]int64{{100000, 100001}, {100001, 100002}, {99998, 100001}} {
		if got := string(weightsRelativeRender(0, flight, body)); got != want {
			t.Errorf("weightsRelativeRender with the flight %v: %s, want %s", flight, got, want)
		}
	}
	for _, flight := range [][2]int64{{100002, 100003}, {99999, 100000}} {
		if got := string(weightsRelativeRender(0, flight, body)); got != string(body) {
			t.Errorf("weightsRelativeRender with the flight %v: %s, want it untouched", flight, got)
		}
	}
}

// dashNormWeightsEnd is the first `before` of a recorded body.
func dashNormWeightsEnd(t *testing.T, body string) int64 {
	t.Helper()
	for _, m := range weightsWindowEndRe.FindAllStringSubmatch(body, -1) {
		if m[1] == "before" {
			var n int64
			if _, err := fmt.Sscan(m[2], &n); err == nil {
				return n
			}
		}
	}
	t.Fatalf("no before in %s", body)
	return 0
}

// dashNormWeightsInterruptedPins pins the 499's judge, its retry rule and its access record reader: the recorded
// attempt shows no problem; a candidate that answers 200, a body after the 499's head, another length, an access
// record of another code or none, and an oracle that is not a 499, carries a body, another length or type, or a record
// of another code are each reported; the row asks again for an oracle's problem only (weightsOracleProblem).
func dashNormWeightsInterruptedPins(t *testing.T) {
	rec := dashNormWeightsInterrupted
	raw := [2][]byte{[]byte(rec.raw[0]), []byte(rec.raw[1])}
	if got := weightsInterruptedJudge(raw, rec.flight, rec.access); got != nil {
		t.Errorf("weightsInterruptedJudge: the recorded attempt: %q", got)
	}
	ok := rec.raw[1]
	for name, c := range map[string]struct {
		candidate, oracle string
		access            [2]string
		want              string
	}{
		"a 200": {strings.Replace(ok, "499 Client Closed Request", "200 OK", 1), rec.raw[0], rec.access,
			"answers differ"},
		"a body": {ok + `{"error": "interrupted" }`, rec.raw[0], rec.access, "answers differ"},
		"another length": {strings.Replace(ok, "Content-Length: 25", "Content-Length: 23", 1), rec.raw[0], rec.access,
			"answers differ"},
		"a 504 record": {ok, rec.raw[0], [2]string{rec.access[0], strings.Replace(rec.access[1], "code=499", "code=504",
			1)}, "access records differ"},
		"no record": {ok, rec.raw[0], [2]string{rec.access[0], ""}, "access records differ"},
		"an oracle's 200": {ok, strings.Replace(rec.raw[0], "499 Client Closed Request", "200 OK", 1), rec.access,
			"oracle: answered"},
		"an oracle's body": {ok, rec.raw[0] + `{"error": "interrupted" }`, rec.access, "oracle: a body came"},
		"an oracle's other length": {ok, strings.Replace(rec.raw[0], "Content-Length: 25", "Content-Length: 23", 1),
			rec.access, "oracle: a 499 without"},
		"an oracle's other type": {ok, strings.Replace(rec.raw[0], "Content-Type: application/json; charset=utf-8",
			"Content-Type: text/plain; charset=utf-8", 1), rec.access, "oracle: a 499 without"},
		"an oracle's 504 record": {ok, rec.raw[0], [2]string{strings.Replace(rec.access[0], "code=499", "code=504", 1),
			rec.access[1]}, "oracle: access record"},
	} {
		got := weightsInterruptedJudge([2][]byte{[]byte(c.oracle), []byte(c.candidate)}, rec.flight, c.access)
		if len(got) == 0 || !strings.HasPrefix(got[0], c.want) {
			t.Errorf("weightsInterruptedJudge: %s gives %q, want %q", name, got, c.want)
		}
		// the row asks again for an oracle's problem only
		if again := weightsOracleProblem(got); again != strings.HasPrefix(c.want, "oracle:") {
			t.Errorf("weightsOracleProblem: %s (%q) asks again: %v", name, got, again)
		}
	}
	if weightsOracleProblem(nil) {
		t.Errorf("weightsOracleProblem: a row without a problem asks again")
	}
	// the reader: one record of the target, reduced to its fields; none or two are no record
	target := weightsHalfCloseTarget(0)
	line := `time=2026-10-09T14:53:06.057Z comm=netdata source=access level=warning tid=3916175 thread=WEB[6] role=any ` +
		`permissions=0x8 src_ip=localhost src_port=34684 req_method=GET code=499 conn=0 transaction=24c8 ` +
		`sent_bytes=25 size_bytes=25 prep_ut=53699 sent_ut=62 total_ut=53761 request="` + target + `"`
	want := `level=warning req_method=GET code=499 sent_bytes=25 size_bytes=25 request="` + target + `"`
	if got := weightsAccess([]string{"other", line}, target); got != want {
		t.Errorf("weightsAccess: %q, want %q", got, want)
	}
	for name, lines := range map[string][]string{
		"none":                    {"other"},
		"two":                     {line, line},
		"another attempt's":       {strings.Replace(line, target, weightsHalfCloseTarget(1), 1)},
		"a record without a code": {strings.Replace(line, " code=499", "", 1)},
	} {
		if got := weightsAccess(lines, target); got != "" {
			t.Errorf("weightsAccess of %s: %q, want none", name, got)
		}
	}
}
