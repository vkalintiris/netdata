// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"maps"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"
)

// dashNormClosersCCases are SA-C's closer rows of `api.v2-alerts` by case, as TestAlertsV2 asks them: the `alerts`
// case's (alertsV2CloserRows, and the search's alertsV2CloserSearchRows), `two-charts`' and `stock-rules`'.
func dashNormClosersCCases() map[string][]v2Req {
	return map[string][]v2Req{
		"alerts":      slices.Concat(alertsV2CloserRows, alertsV2CloserSearchRows),
		"two-charts":  alertsV2TwoChartsRows(),
		"stock-rules": alertsV2StockRulesRows(),
	}
}

// dashNormClosersCPairOf normalises the bodies o and c as row r's check does: the search's family for a row recorded
// without an alert log (alertsV2CloserSearchRows), else alertsV2Family with each side's recorded log and a
// normalizer that has seen nothing. A body that does not parse is a failure of the pin.
func dashNormClosersCPairOf(t *testing.T, r dashNormClosersCRow, o, c string) dashNormAlertsPair {
	t.Helper()
	var p dashNormAlertsPair
	if r.logs[0] == "" {
		p.fam = searchFamily
	} else {
		p.fam = alertsV2Family([2]*healthNorm{dashNormAlertsNorm(), dashNormAlertsNorm()},
			func(i int) ([]byte, error) { return []byte(dashNormClosersCLogs[r.logs[i]]), nil })
	}
	for i, b := range []string{o, c} {
		p.body[i] = p.fam.normalise(i, r.flight[i], r.flight[i], []byte(b))
		v, err := ParseJSON(p.body[i])
		if err != nil {
			t.Fatalf("%v: %s", err, p.body[i])
		}
		p.doc[i] = v
	}
	return p
}

// testDashNormClosersC pins SA-C's closer rows of `api.v2-alerts` on the answers and alert logs of one C-against-C
// run (dash_norm_closers_c_data_test.go): each row's guard and family on its own recorded pair, what each guard
// refuses, what the render of `values` without `instances` names and leaves, and planted wrong answers.
func testDashNormClosersC(t *testing.T) {
	guards, targets := map[string]func(Value) error{}, map[string]string{}
	for name, rows := range dashNormClosersCCases() {
		for _, req := range rows {
			if req.guard == nil || req.status != "200" {
				t.Errorf("%s/%s: a row without a guard, or one that wants another status than 200", name, req.name)
			}
			if _, twice := guards[name+"/"+req.name]; twice {
				t.Errorf("%s/%s: two rows of one name", name, req.name)
			}
			guards[name+"/"+req.name], targets[name+"/"+req.name] = req.guard, req.target
		}
	}
	// no closer row has the name of a row of its case before it: a failure names its row
	for name, rows := range dashNormAlertsCases() {
		for _, req := range rows {
			if _, both := guards[name+"/"+req.name]; both {
				t.Errorf("%s/%s: a closer row has the name of one of the case's rows", name, req.name)
			}
		}
	}
	// every row was recorded, and nothing else
	if got, want := slices.Sorted(maps.Keys(dashNormClosersCRows)), slices.Sorted(maps.Keys(guards)); !slices.Equal(got, want) {
		t.Fatalf("the recorded rows are %q, the checks' rows %q", got, want)
	}

	// Each recorded pair shows no difference under its family, the seconds its render named are within the bound,
	// and the row's guard takes the oracle's answer as the check hands it over: normalised.
	pairs := map[string]dashNormAlertsPair{}
	for _, key := range slices.Sorted(maps.Keys(dashNormClosersCRows)) {
		r := dashNormClosersCRows[key]
		p := dashNormClosersCPairOf(t, r, r.bodies[0], r.bodies[1])
		pairs[key] = p
		if got := p.diffs(); got != "" {
			t.Errorf("%s: the recorded pair differs at %q", key, got)
		}
		if p.fam.check != nil {
			p.fam.check(t, key, p.doc[0], p.doc[1])
		}
		if err := guards[key](p.doc[0]); err != nil {
			t.Errorf("%s: its guard refuses C's answer: %v", key, err)
		}
		if targets[key] != r.target {
			t.Errorf("%s asks %q, its answer was recorded for %q", key, targets[key], r.target)
		}
	}

	// What each guard refuses: the answer of every other row of its case, but the rows named here, whose answers hold
	// what the guard reads.
	accepted := map[string][]string{}
	for key, guard := range guards {
		name, _, _ := strings.Cut(key, "/")
		for other, p := range pairs {
			if other != key && strings.HasPrefix(other, name+"/") && guard(p.doc[0]) == nil {
				accepted[key] = append(accepted[key], strings.TrimPrefix(other, name+"/"))
			}
		}
		slices.Sort(accepted[key])
	}
	if want := dashNormClosersCAccepted; !maps.EqualFunc(accepted, want, slices.Equal[[]string]) {
		for _, key := range slices.Sorted(maps.Keys(guards)) {
			if !slices.Equal(accepted[key], want[key]) {
				t.Errorf("%s: its guard also takes the answers of %q, want %q", key, accepted[key], want[key])
			}
		}
	}

	dashNormClosersCRender(t, pairs)
	dashNormClosersCPlanted(t, guards)
	dashNormClosersCHelpers(t)
}

// dashNormClosersCHelpers pins the `two-charts` case's anchor guard (alertsV2LogTimes): each line exactly n times.
func dashNormClosersCHelpers(t *testing.T) {
	t.Helper()
	guard := alertsV2LogTimes(2, "a: UNINITIALIZED->WARNING 1 things", "b: UNINITIALIZED->CLEAR 1 things")
	for view, bad := range map[string]bool{
		"a: UNINITIALIZED->WARNING 1 things\na: UNINITIALIZED->WARNING 1 things\nb: UNINITIALIZED->CLEAR 1 things\n" +
			"b: UNINITIALIZED->CLEAR 1 things": false,
		"a: REMOVED->UNINITIALIZED -\na: UNINITIALIZED->WARNING 1 things\na: UNINITIALIZED->WARNING 1 things\n" +
			"b: UNINITIALIZED->CLEAR 1 things\nb: UNINITIALIZED->CLEAR 1 things": false,
		"a: UNINITIALIZED->WARNING 1 things\nb: UNINITIALIZED->CLEAR 1 things\nb: UNINITIALIZED->CLEAR 1 things": true,
		"a: UNINITIALIZED->WARNING 1 things\na: UNINITIALIZED->WARNING 1 things\na: UNINITIALIZED->WARNING 1 things\n" +
			"b: UNINITIALIZED->CLEAR 1 things\nb: UNINITIALIZED->CLEAR 1 things": true,
		"a: UNINITIALIZED->WARNING 1 things\na: UNINITIALIZED->WARNING 1 things": true,
		"a: UNINITIALIZED->WARNING 1 things x\na: UNINITIALIZED->WARNING 1 things x\nb: UNINITIALIZED->CLEAR 1 things\n" +
			"b: UNINITIALIZED->CLEAR 1 things": true,
	} {
		if err := guard(view); (err != nil) != bad {
			t.Errorf("the anchor's guard on %q: %v", view, err)
		}
	}
}

// dashNormClosersCAccepted are, per row, the other rows of its case whose recorded answers its guard takes too.
var dashNormClosersCAccepted = map[string][]string{
	// the three spellings of one status list
	"alerts/status-blank": {"status-pipe", "status-plus"},
	"alerts/status-pipe":  {"status-blank", "status-plus"},
	"alerts/status-plus":  {"status-blank", "status-pipe"},
	// every alert with these options: no status word, and a limit the plain form does not read
	"alerts/plain-cardinality": {"status-upper"},
	"alerts/status-upper":      {"plain-cardinality"},
}

// dashNormClosersCRender pins what alertsV2RenderValues wrote into the recorded oracle answers of `values` without
// `instances`, plain and MCP (alertsV2RenderMCP's), what it left, and the render on bodies of its own.
func dashNormClosersCRender(t *testing.T, pairs map[string]dashNormAlertsPair) {
	t.Helper()
	for key, wants := range map[string][]string{
		"alerts/values-alone": {`"nm":"hm_plain","ch":"hsig.plain","ch_n":"hsig.plain","v":null,"t":0}`,
			`"nm":"ha_low","ch":"hsig.values","ch_n":"hsig.values","v":70,"t":"T"}`},
		"alerts/values-summary":  {`"ch_n":"hsig.values","v":70,"t":"T"}`},
		"alerts/mcp-values-only": {`["hm_plain","parity-parent","hsig.plain",null,0]`, `["ha_low","parity-parent","hsig.values",70,"T"]`},
		"two-charts/values":      {`"ch_n":"hcl.shown","v":null,"t":0}`, `"ch":"hcl.values","ch_n":"hcl.values","v":1,"t":"T"}`},
	} {
		for _, want := range wants {
			if got := string(pairs[key].body[0]); !strings.Contains(got, want) {
				t.Errorf("%s, rendered, does not hold %s: %s", key, want, got)
			}
		}
	}
	// what the render leaves: an answer without an instance, byte for byte
	for _, key := range []string{"alerts/debug-rfc3339", "alerts/active-debug", "two-charts/summary", "stock-rules/groupings"} {
		if got := string(pairs[key].body[0]); got != dashNormClosersCRows[key].bodies[0] {
			t.Errorf("%s: the render changed an answer that holds no instance: %s", key, got)
		}
	}
	// on bodies of its own, asked in the seconds [100, 102]: a second of the flight or of the healthBound seconds
	// before it is named, by the short key and the long one, as a number or a date; 0, null, the second before those
	// and the second after the flight stay; a body with a global id, or of the MCP form, is not read
	flight := [2]int64{100, 102}
	date := func(sec int64) string { return time.Unix(sec, 0).UTC().Format(`"2006-01-02T15:04:05Z"`) }
	for in, want := range map[string]string{
		`{"alert_instances":[{"t":100},{"t":98},{"t":102}]}`:                   `{"alert_instances":[{"t":"T"},{"t":"T"},{"t":"T"}]}`,
		`{"alert_instances":[{"t":97},{"t":103},{"t":0},{"t":null}]}`:          `{"alert_instances":[{"t":97},{"t":103},{"t":0},{"t":null}]}`,
		`{"alert_instances":[{"last_updated_timestamp":101}]}`:                 `{"alert_instances":[{"last_updated_timestamp":"T"}]}`,
		`{"alert_instances":[{"t":` + date(101) + `},{"t":` + date(97) + `}]}`: `{"alert_instances":[{"t":"T` + alertsV2Dated + `"},{"t":` + date(97) + `}]}`,
		`{"alert_instances":[{"t": 101}]}`:                                     `{"alert_instances":[{"t": "T"}]}`,
		`{"alert_instances":[{"gi":100000000,"t":101}]}`:                       `{"alert_instances":[{"gi":100000000,"t":101}]}`,
		`{"alert_instances_header":["x"],"alert_instances":[["a",{"t":101}]]}`: `{"alert_instances_header":["x"],"alert_instances":[["a",{"t":101}]]}`,
		`{"all_alerts_header":["x"],"alert_instances":[{"t":101}]}`:            `{"all_alerts_header":["x"],"alert_instances":[{"t":101}]}`,
		`{"alert_instances":[{"tt":101,"t_":101}]}`:                            `{"alert_instances":[{"tt":101,"t_":101}]}`,
	} {
		got, clocks := alertsV2RenderValues(flight, []byte(in))
		if string(got) != want {
			t.Errorf("the values render of %s: %s, want %s", in, got, want)
		}
		if n := strings.Count(string(got), `"T`); len(clocks) != n {
			t.Errorf("the values render of %s returned %d seconds for %d marks", in, len(clocks), n)
		}
	}
	if _, clocks := alertsV2RenderValues(flight, []byte(`{"alert_instances":[{"t":98},{"t":102}]}`)); !slices.Equal(clocks, []int64{98, 102}) {
		t.Errorf("the values render returned the seconds %v, want [98 102]", clocks)
	}
	// a 0 is never a second of the flight, also where the bound before the flight reaches it
	if got, _ := alertsV2RenderValues([2]int64{1, 2}, []byte(`{"alert_instances":[{"t":0},{"t":1}]}`)); string(got) != `{"alert_instances":[{"t":0},{"t":"T"}]}` {
		t.Errorf("the values render in the seconds [1, 2]: %s", got)
	}
	// alertsV2Render hands a body without a global id to it: the family's bound holds what it named
	fam := alertsV2Family([2]*healthNorm{dashNormAlertsNorm(), dashNormAlertsNorm()}, func(int) ([]byte, error) { return []byte(`[]`), nil })
	if got := string(fam.render(0, flight, []byte(`{"alert_instances":[{"t":101}]}`))); got != `{"alert_instances":[{"t":"T"}]}` {
		t.Errorf("the alerts family's render of an instance without a global id: %s", got)
	}
	if _, near := alertsV2Render(dashNormAlertsNorm(), alertsV2Log{}, flight, []byte(`{"alert_instances":[{"t":101}]}`)); !slices.Equal(near, []int64{101}) {
		t.Errorf("alertsV2Render returned the seconds %v for an instance without a global id, want [101]", near)
	}
	_, near := alertsV2RenderValues(flight, []byte(`{"alert_instances":[{"t":101}]}`))
	_, far := alertsV2RenderValues([2]int64{104, 104}, []byte(`{"alert_instances":[{"t":104}]}`))
	if healthClocksNear(near, far) == nil {
		t.Errorf("three seconds apart, the bound holds: %v, %v", near, far)
	}
}

// dashNormClosersCPlanted plants each wrong answer in a recorded body: in the candidate's, where the comparison must
// report it at its paths and nowhere else (an empty want: no difference), and, when `refused`, in the oracle's, which
// the row's guard must refuse. A plant is a pattern with three groups, the second of which `to` rewrites.
func dashNormClosersCPlanted(t *testing.T, guards map[string]func(Value) error) {
	t.Helper()
	text := func(s string) func(string) string { return func(string) string { return s } }
	// the second off the edge of the row's flight on the candidate's side: what an agent that evaluated then prints
	flightAt := func(row string, edge int, off int64) func(string) string {
		return func(string) string { return strconv.FormatInt(dashNormClosersCRows[row].flight[1][edge]+off, 10) }
	}
	swapped := func(pair string) string {
		a, b, _ := strings.Cut(pair, ",")
		return b + "," + a
	}
	// two objects of a list, `{a},{b}`, the other way round
	objects := func(pair string) string {
		a, b, _ := strings.Cut(pair, "},{")
		return "{" + b + "," + a + "}"
	}
	const lowValues = `("nm":"ha_low","ch":"hsig.values","ch_n":"hsig.values","v":70,"t":)(\d+)(\})`
	plants := map[string]struct {
		row, re string
		to      func(string) string
		want    string
		refused bool
	}{
		// `values` without `instances`: the last evaluation by the request's flight, as alertsV2Render reads it
		"values: evaluated at the request's first second":    {"alerts/values-alone", lowValues, flightAt("alerts/values-alone", 0, 0), "", false},
		"values: evaluated two seconds before the request":   {"alerts/values-alone", lowValues, flightAt("alerts/values-alone", 0, -healthBound), "", false},
		"values: evaluated three seconds before the request": {"alerts/values-alone", lowValues, flightAt("alerts/values-alone", 0, -healthBound-1), "$.alert_instances[1].t", true},
		"values: evaluated a second after the request":       {"alerts/values-alone", lowValues, flightAt("alerts/values-alone", 1, 1), "$.alert_instances[1].t", true},
		"values: a second for an alert never evaluated": {"alerts/values-uninitialized", `("nm":"hm_plain"[^}]*"t":)(0)(\})`,
			flightAt("alerts/values-uninitialized", 0, 0), "$.alert_instances[0].t", true},
		"values: a global id without `instances`": {"alerts/values-uninitialized", `(\{"ni":0,)()("nm":"hm_plain")`, text(`"gi":1,`), "$.alert_instances[0].<members> layout", true},
		"values: a status without `instances`": {"alerts/values-alone", `("ch_n":"hsig.plain",)()("v":null)`, text(`"st":"UNINITIALIZED",`),
			"$.alert_instances[0].<members> layout", true},
		"values: an index without `summary`": {"alerts/values-uninitialized", `(\{)()("ni":0,"nm":"hm_plain")`, text(`"ati":0,`), "$.alert_instances[0].<members> layout", true},
		"values: no index beside `summary`":  {"alerts/values-summary", `("alert_instances":\[\{)("ati":0,)("ni":0)`, text(""), "$.alert_instances[0].<members> layout", true},
		"values: the chart's id for its name": {"two-charts/values", `("ch":"hcl\.named","ch_n":")(hcl\.shown)(")`, text("hcl.named"),
			"$.alert_instances[0].ch_n", true},
		"instances: the chart's id for its name": {"two-charts/instances", `("ch":"hcl\.named","ch_n":")(hcl\.shown)(")`, text("hcl.named"),
			"$.alert_instances[0].ch_n", true},
		"mcp: the chart's id for its name": {"two-charts/mcp-values", `(\["hcl_named","parity-parent",")(hcl\.shown)(")`, text("hcl.named"),
			"$.alert_instances[0][2]", true},
		"mcp: evaluated a second after the request": {"alerts/mcp-values-only", `(\["ha_low","parity-parent","hsig\.values",70,)(\d+)(\])`,
			flightAt("alerts/mcp-values-only", 1, 1), "$.alert_instances[1][4]", true},
		"mcp: the values beside instances alone": {"alerts/mcp-instances-only", `("Workload")()(\])`, text(`,null,0`), "$.alert_instances[0].length layout", true},
		// a header text other than C's, by one option alone (the render reads the rows by position, not by header)
		"mcp: a header text of instances alone": {"alerts/mcp-instances-only", `("Component",")(Classification)("\])`, text("Class"),
			"$.alert_instances_header[17]", true},
		"mcp: a header text of values alone": {"alerts/mcp-values-only", `("Last Updated Value",")(Last Updated Timestamp)("\])`,
			text("Last Updated Time"), "$.alert_instances_header[4]", true},
		// the counts above 1 and the silent recipient (the planted bugs m07 and m09 of commit 4's teeth)
		"summary: a warning counter set, not added to": {"two-charts/summary", `("nm":"hcl_up"[^}]*"wr":)(2)(,)`, text("1"), "$.alerts[1].wr", true},
		"summary: a clear counter set, not added to":   {"two-charts/summary", `("nm":"hcl_flat"[^}]*"cl":)(2)(,)`, text("1"), "$.alerts[2].cl", true},
		"summary: silent by its first letters": {"two-charts/summary", `("name":"silent sysadmin"[^}]*"running_silent":)(0)(,)`, text("2"),
			"$.alerts_by_recipient[1].running_silent", true},
		"summary: silent by its first letters, by type": {"two-charts/summary", `("name":"Closer Check"[^}]*"running_silent":)(0)(,)`,
			text("2"), "$.alerts_by_type[0].running_silent", true},
		"summary: a rule left out has no prototype": {"two-charts/summary", `(\{"name":"Closer Check"[^}]*\})(,\{"name":"Off Kind"[^}]*\})(\])`,
			text(""), "$.alerts_by_type.length layout", true},
		"mcp summary: warnings and clears swapped": {"two-charts/mcp-summary", `(\["hcl_up",[^\]]*"silent_sysadmin",0,)(2,0)(,)`,
			swapped, "$.all_alerts[1][8] $.all_alerts[1][9]", true},
		// the status list, the echo, the plain form's limit, the route and the search
		"echo: the status word as sent":   {"alerts/active-debug", `("status":\[")(raised)("\])`, text("active"), "$.request.selectors.alerts.status[0]", true},
		"echo: a word twice":              {"alerts/debug-statuses", `("clear",)("raised")(\])`, text(`"raised","raised"`), "$.request.selectors.alerts.status.length layout", true},
		"echo: rfc3339 not read":          {"alerts/debug-rfc3339", `("before":)(null)(\s)`, text("0"), "$.request.filters.before", true},
		"echo: a relative time as a date": {"alerts/debug-rfc3339", `("after":)(-600)(,)`, text(`"1970-01-01T00:00:00Z"`), "$.request.filters.after", true},
		"plain form: cut at the limit": {"alerts/plain-cardinality", `("alerts":\[\{"ati":0,[^}]*\})((?:,\{"ati":\d,[^}]*\})+)(\])`, text(""),
			"$.alerts.length layout", true},
		"v2: the plain form's api member": {"alerts/v2-mcp", `(\{)()("nodes")`, text(`"api":2,`), "$.<members> layout", true},
		"search: the alert's context found": {"alerts/q-alert", `("contexts":\{)(\s*)(\})`,
			text(`"hsig.ctx":{"matched":["alerts"]}`), "$.contexts.<members> layout", true},
		// what a search that tested the four alerts' names too would count
		"search: the alerts' names tested too": {"alerts/q-alert", `("searches":\{\s*"strings":)(8)(,)`, text("12"), "$.searches.strings", true},
		"stock rules: two entries the other way round": {"stock-rules/groupings",
			`("alerts_by_type":\[)(\{[^}]*\},\{[^}]*\})(,)`, objects, "$.alerts_by_type[0].name $.alerts_by_type[0].available $.alerts_by_type[1].name $.alerts_by_type[1].available", true},
		"stock rules: the first two components the other way round": {"stock-rules/groupings",
			`("alerts_by_component":\[)(\{[^}]*\},\{[^}]*\})(,)`, objects, "$.alerts_by_component[0].name $.alerts_by_component[0].available $.alerts_by_component[1].name $.alerts_by_component[1].available", true},
		"stock rules: a component less": {"stock-rules/groupings", `(\})(,\{"name":"[^"]*"[^}]*\})(\],"alerts_by_classification")`,
			text(""), "$.alerts_by_component.length layout", true},
	}
	for _, name := range slices.Sorted(maps.Keys(plants)) {
		c := plants[name]
		r, ok := dashNormClosersCRows[c.row]
		if !ok {
			t.Fatalf("%s: no recorded row %s", name, c.row)
		}
		planted := dashNormAlertsSub(t, r.bodies[1], c.re, func(g []string) string { return g[1] + c.to(g[2]) + g[3] })
		if got := dashNormClosersCPairOf(t, r, r.bodies[0], planted).diffs(); got != c.want {
			t.Errorf("%s: differences at %q, want %q", name, got, c.want)
		}
		if !c.refused {
			continue
		}
		wrong := dashNormAlertsSub(t, r.bodies[0], c.re, func(g []string) string { return g[1] + c.to(g[2]) + g[3] })
		if err := guards[c.row](dashNormClosersCPairOf(t, r, wrong, r.bodies[1]).doc[0]); err == nil {
			t.Errorf("%s: the guard of %s takes it", name, c.row)
		}
	}

	// What a wrong parser of the status list answers, as C answered the same options for another status (H36's
	// recorded rows, dash_norm_alerts_rows_data_test.go): `RAISED` read without the case is `active`'s answer; a list
	// not split at its separator (`clear|warning` one word, which is none) is `bogus`'s, and one cut at its first
	// separator is `clear`'s. Each closer row's guard refuses them.
	for row, wrongs := range map[string][]string{
		"alerts/status-upper": {"alerts/active"},
		"alerts/status-pipe":  {"alerts/bogus", "alerts/clear", "alerts/active"},
		"alerts/status-blank": {"alerts/bogus", "alerts/clear", "alerts/active"},
		"alerts/status-plus":  {"alerts/bogus", "alerts/clear", "alerts/active"},
	} {
		for _, wrong := range wrongs {
			r := dashNormAlertsRows[wrong]
			if !strings.HasSuffix(r.target, "?options=summary,instances,minify&status="+strings.TrimPrefix(wrong, "alerts/")) {
				t.Fatalf("%s was recorded for %q: not the options of %s", wrong, r.target, row)
			}
			if err := guards[row](dashNormAlertsPairOf(t, r, r.bodies[0], r.bodies[1]).doc[0]); err == nil {
				t.Errorf("%s: its guard takes C's answer to %s", row, r.target)
			}
		}
	}
}
