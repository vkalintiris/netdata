// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"net/url"
	"regexp"
	"strings"
	"testing"
)

// healthVarEmit are the variables check's charts that are defined and never collected, and its custom variables
// (plugins.d/pluginsd_parser.c:667-749, :841-899); the collected chart is `hv.a` (context `hv.ctx`, `a` 10, `b` 20):
//   - `hv.b`: a name that is not its id, context `hv.ctx`, an update every of 5 s, a label, chart variables: `cv`;
//     `cv_raw` (the suffix of a dimension's raw value); `hv.b.dv` (a dotted name); `a` (its dimension's name); `c+v`,
//     which C stores as `c_v` (rrdvar.c:12-18);
//   - `hv.c`: context `hv.ctx`, another value of the label, a dimension whose name is not its id and one with a dot;
//   - `hv.d`: no context, so its context is its id;
//   - `hv.e`: a context without a dot;
//   - host variables: `hvh`; `cv` (hv.b's chart variable's name); `status` (a name the resolver knows).
const healthVarEmit = `CHART hv.b bname 'title' 'units' 'family' 'hv.ctx' line 1000 5 '' '' ''
DIMENSION a '' absolute 1 1
CLABEL kind x 1
CLABEL_COMMIT
VARIABLE CHART cv = 7
VARIABLE CHART cv_raw = 8
VARIABLE CHART hv.b.dv = 9
VARIABLE CHART a = 6
VARIABLE CHART c+v = 4
CHART hv.c '' 'title' 'units' 'family' 'hv.ctx' line 1000 1 '' '' ''
DIMENSION a aname absolute 1 1
DIMENSION p.q '' absolute 1 1
CLABEL kind y 1
CLABEL_COMMIT
CHART hv.d '' 'title' 'units' 'family' '' line 1000 1 '' '' ''
DIMENSION a '' absolute 1 1
CHART hv.e '' 'title' 'units' 'family' 'hvflat' line 1000 1 '' '' ''
DIMENSION a '' absolute 1 1
VARIABLE HOST hvh = 5
VARIABLE HOST cv = 55
VARIABLE HOST status = 77
`

// healthVarConf are the `on` case's rules. C links each and never runs it: a lookup over a day needs a chart whose
// data began a day ago (health_event_loop.c:173-187), so every alert stays UNINITIALIZED with no value on both sides.
// hv_same is linked to the three charts of `hv.ctx`, in the order they were created: hv.b, hv.c, hv.a.
const healthVarConf = `# the variables check's rules: linked, never run
template: hv_same
      on: hv.ctx
  lookup: average -1d of a
   every: 1s
   units: things

   alarm: hv_one
      on: hv.a
  lookup: average -1d of a
   every: 1s
   units: things
`

// healthVarTrace is a trace the variables check asks for: a variable as an alert of the chart would resolve it
// (health_variable.c:290-571), and what the oracle must answer (the guard; the comparison is of the bytes): `-` (not
// found), or the value, the source's description, the chart it came from and the number of candidates. A clock's
// second is `T`.
type healthVarTrace struct {
	chart, variable, want string
}

const (
	hvStored = "last stored value of dimension"
	hvRaw    = "last collected value of dimension"
	hvTime   = "last collected time of dimension"
	hvChart  = "chart variable"
	hvHost   = "host variable"
)

// healthVarTraces are the traces of both cases: nothing in them depends on an alert.
var healthVarTraces = []healthVarTrace{
	// the collected chart's dimensions: the stored value, the collected one, the second of the last collection
	{"hv.a", "a", "10|" + hvStored + "|hv.a|1"},
	{"hv.a", "a_raw", "10|" + hvRaw + "|hv.a|1"},
	{"hv.a", "a_last_collected_t", "T|" + hvTime + "|hv.a|1"},
	{"hv.a", "b", "20|" + hvStored + "|hv.a|1"},
	// names are whole and case-sensitive
	{"hv.a", "A", "-"},
	{"hv.a", "nope", "-"},
	// what the resolver knows by name: a trace runs on a blank alert, whose value, status and window are 0
	{"hv.a", "this", "0|current alert value|hv.a|1"},
	{"hv.a", "status", "0|current alert status|hv.a|1"},
	{"hv.a", "after", "0|current alert query start time|hv.a|1"},
	{"hv.a", "before", "0|current alert query end time|hv.a|1"},
	{"hv.a", "now", "T|current wall-time clock timestamp|hv.a|1"},
	{"hv.a", "REMOVED", "-2|removed status constant|hv.a|1"},
	{"hv.a", "UNINITIALIZED", "0|uninitialized status constant|hv.a|1"},
	{"hv.a", "UNDEFINED", "-1|undefined status constant|hv.a|1"},
	{"hv.a", "CLEAR", "1|clear status constant|hv.a|1"},
	{"hv.a", "WARNING", "3|warning status constant|hv.a|1"},
	{"hv.a", "CRITICAL", "4|critical status constant|hv.a|1"},
	{"hv.a", "critical", "-"},
	// the chart's own update every and last collection (0: never collected)
	{"hv.a", "update_every", "1|current instance update_every|hv.a|1"},
	{"hv.b", "update_every", "5|current instance update_every|hv.b|1"},
	{"hv.a", "last_collected_t", "T|current instance last_collected_t|hv.a|1"},
	{"hv.b", "last_collected_t", "0|current instance last_collected_t|hv.b|1"},
	// custom variables: the chart's before the host's; another chart's are not this chart's; the name as C stored it
	{"hv.a", "hvh", "5|" + hvHost + "|hv.a|1"},
	{"hv.a", "cv", "55|" + hvHost + "|hv.a|1"},
	{"hv.b", "cv", "7|" + hvChart + "|hv.b|1"},
	{"hv.b", "c_v", "4|" + hvChart + "|hv.b|1"},
	{"hv.a", "c_v", "-"},
	// a chart variable is asked for by its whole name: `cv_raw` is not the raw value of a dimension `cv`
	{"hv.b", "cv_raw", "8|" + hvChart + "|hv.b|1"},
	// a dimension before a chart variable of its name (hv.b's `a` is 6; a dimension never collected holds 0); a
	// dimension by its name, and one with a dot in its id
	{"hv.b", "a", "0|" + hvStored + "|hv.b|1"},
	{"hv.c", "aname", "0|" + hvStored + "|hv.c|1"},
	{"hv.c", "aname_raw", "0|" + hvRaw + "|hv.c|1"},
	{"hv.c", "p.q", "0|" + hvStored + "|hv.c|1"},
	// CHART.DIM, with the suffixes; the chart is found by its id, not by its name
	{"hv.b", "hv.a.a", "10|" + hvStored + "|hv.a|1"},
	{"hv.b", "hv.a.a_raw", "10|" + hvRaw + "|hv.a|1"},
	{"hv.b", "hv.a.a_last_collected_t", "T|" + hvTime + "|hv.a|1"},
	{"hv.a", "hv.b.a", "0|" + hvStored + "|hv.b|1"},
	{"hv.a", "hv.a.nope", "-"},
	{"hv.a", "hv.bname.a", "-"},
	// the split moves one dot to the left: chart hv.c, dimension p.q
	{"hv.a", "hv.c.p.q", "0|" + hvStored + "|hv.c|1"},
	// a name with one dot is not split: `hvflat` is hv.e's context
	{"hv.a", "hvflat.a", "-"},
	// another chart's variable is asked for by the whole dotted name: hv.b has `cv` and `hv.b.dv`
	{"hv.a", "hv.b.cv", "-"},
	{"hv.a", "hv.b.dv", "9|" + hvChart + "|hv.b|1"},
	{"hv.b", "hv.b.dv", "9|" + hvChart + "|hv.b|1"},
	// CONTEXT.DIM: every chart of the context is a candidate; the one sharing the most labels with the asking chart
	// wins, the first of them on a tie. hv.a has no label of its own: a tie, and hv.b, created first, wins with its 0
	// over hv.a's own 10; hv.c shares its three labels with itself, and wins although hv.b comes first
	{"hv.a", "hv.ctx.a", "0|" + hvStored + "|hv.b|3"},
	{"hv.b", "hv.ctx.a", "0|" + hvStored + "|hv.b|3"},
	{"hv.c", "hv.ctx.a", "0|" + hvStored + "|hv.c|3"},
	{"hv.c", "hv.ctx.b", "20|" + hvStored + "|hv.a|1"},
	// a chart whose context is its id is found twice
	{"hv.a", "hv.d.a", "0|" + hvStored + "|hv.d|2"},
}

// healthVarTraceRe reads a trace's view: found, and when found the value and the source's four members.
var healthVarTraceRe = regexp.MustCompile(`"found":(true|false)(?:,\s*"value":([^,\s]+),\s*"source":\{\s*"description":"([^"]*)",\s*` +
	`"instance":"([^"]*)",\s*"context":"[^"]*",\s*"candidates":(\d+)\s*\})?\s*\}\s*$`)

// healthVarTraced is a guard on a trace's view: the answer is 200, about the asked chart and variable, and resolves
// as `want` says.
func healthVarTraced(c healthVarTrace) func(string) error {
	return func(view string) error {
		if err := healthHolds(fmt.Sprintf(`"variable":%q`, c.variable))(view); err != nil {
			return err
		}
		m := healthVarTraceRe.FindStringSubmatch(view)
		got := "(not a trace)"
		switch {
		case m == nil:
		case m[1] == "false":
			got = "-"
		default:
			got = strings.Join(m[2:], "|")
		}
		if got != c.want {
			return fmt.Errorf("the trace resolves as %q, want %q", got, c.want)
		}
		return nil
	}
}

// healthVarPath is the request of a trace.
func healthVarPath(version int, chart, variable string) string {
	return fmt.Sprintf("/api/v%d/variable?chart=%s&variable=%s", version, url.QueryEscape(chart), url.QueryEscape(variable))
}

// TestHealthVariables (check `health.variables`, M9 commit 3, D194 F1): the two endpoints of an alert's variables, on
// five charts of the fake plugin: one collected, four defined and never collected (healthVarEmit), with custom chart
// and host variables. `/api/v1/variable` is C's resolver traced into JSON (health_variable.c:577-596) and
// `/api/v1/alarm_variables` lists what a chart's alerts can name (rrdvar.c:159-320); neither asks whether health
// runs (web/api/v1/api_v1_alarms.c:104-156). Each answer's status, content type and body bytes are compared, the
// clock's seconds masked (healthVars, healthTrace) and bound side to side (healthClocksNear). Cases:
//   - `off`: health off. `alarm_variables` of the collected chart, of a chart never collected (by its id and by its
//     name) and of one whose dimension has a name; the traces of healthVarTraces; a trace by the chart's name and
//     through `/api/v3/variable`; the five error answers; the `chart_variables` member of `/api/v1/chart`;
//   - `on`: health on with two rules C links and never runs (healthVarConf): `alarm_variables` with its `alerts`
//     (per name the alert whose chart shares the most labels with the asked chart, the first on a tie), the traces
//     of the alerts' names, and the traces of `off` again.
func TestHealthVariables(t *testing.T) {
	vars := func(t *testing.T, h *healthPair, chart string, parts ...string) {
		t.Helper()
		path := "/api/v1/alarm_variables?chart=" + chart
		h.compareNow(t, path, func(i int) string { return h.vars(i, path) }, healthHolds(parts...))
	}
	traces := func(t *testing.T, h *healthPair, list []healthVarTrace) {
		t.Helper()
		for _, c := range list {
			path := healthVarPath(1, c.chart, c.variable)
			h.compareNow(t, path, func(i int) string { return h.trace(i, path) }, healthVarTraced(c))
		}
	}
	// an answer that is no JSON: its status, content type and text
	text := func(t *testing.T, h *healthPair, path string, status int, body string) {
		t.Helper()
		h.compareNow(t, path, func(i int) string { return h.plain(i, path) }, func(view string) error {
			if got := healthBody(view); !strings.HasPrefix(view, fmt.Sprintf("HTTP %d, ", status)) || got != body {
				return fmt.Errorf("answered %q with %q, want %d with %q", strings.SplitN(view, "\n", 2)[0], got, status, body)
			}
			return nil
		})
	}
	// the collected chart shows its values once two collections were stored: the first comparison waits for them
	collected := []string{`"chart":"hv.a"`, `"chart_name":"hv.a"`, `"chart_context":"hv.ctx"`, `"after":T,`, `"before":T+1,`,
		`"now":T+1,`, `"a":10,`, `"b":20`, `"a_raw":10,`, `"b_raw":20`, `"a_last_collected_t":T,`, `"update_every":1,`,
		`"last_collected_t":T`, `"hvh":5,`, `"cv":55,`, `"status":77`}
	// a chart that is never collected: a stored value, a collected value and the times of 0
	never := []string{`"chart":"hv.b"`, `"chart_name":"hv.bname"`, `"a":0`, `"a_raw":0`, `"a_last_collected_t":0`,
		`"update_every":5,`, `"last_collected_t":0,`, `"cv":7,`, `"cv_raw":8,`, `"hv.b.dv":9,`, `"a":6,`, `"c_v":4`}
	// no alert: the member as C prints an empty object
	none := "\"alerts\":{\n    }"
	// an entry of `alerts`: the alert of that name on a chart, and how many labels that chart shares with the asked one
	alert := func(name, chart string, score int) string {
		return fmt.Sprintf("%q:{\n            \"value\":null,\n            \"instance\":%q,\n            \"context\":\"hv.ctx\",\n"+
			"            \"score\":%d\n        }", name, chart, score)
	}
	sc := healthScenario(healthVarEmit, "hv.a", "hv.ctx", []string{"a", "b"}, map[string]int64{"a": 10, "b": 20})
	runHealthCases(t, map[string]healthCase{
		"off": {
			off: true,
			sc:  sc,
			play: func(t *testing.T, h *healthPair) {
				h.createChart(t)
				vars(t, h, "hv.a", append(collected, none)...)
				vars(t, h, "hv.b", append(never, none)...)
				// a chart by its name
				vars(t, h, "hv.bname", append(never, none)...)
				// a dimension's name beside its id: `a` and `aname`
				vars(t, h, "hv.c", `"chart":"hv.c"`, `"a":0,`, `"aname":0,`, `"p.q":0`, `"aname_raw":0,`, `"aname_last_collected_t":0,`,
					none)
				traces(t, h, healthVarTraces)
				byName := healthVarPath(1, "hv.bname", "cv")
				h.compareNow(t, byName, func(i int) string { return h.trace(i, byName) },
					healthVarTraced(healthVarTrace{"hv.bname", "cv", "7|" + hvChart + "|hv.b|1"}))
				v3 := healthVarPath(3, "hv.a", "a")
				h.compareNow(t, v3, func(i int) string { return h.trace(i, v3) },
					healthVarTraced(healthVarTrace{"hv.a", "a", "10|" + hvStored + "|hv.a|1"}))
				// the errors: no chart, no variable, a chart nobody has (its name HTML-escaped)
				text(t, h, "/api/v1/alarm_variables", 400, "No chart id is given at the request.")
				text(t, h, "/api/v1/alarm_variables?chart=no%3Cchart", 404, "Chart is not found: no&lt;chart")
				text(t, h, "/api/v1/variable?chart=hv.a", 400, "A chart= and a variable= are required.")
				text(t, h, "/api/v1/variable?variable=a", 400, "A chart= and a variable= are required.")
				text(t, h, "/api/v1/variable?chart=no%3Cchart&variable=a", 404, "Chart is not found: no&lt;chart")
				// the chart's own JSON: its custom variables, without health
				for _, c := range [][2]string{{"hv.b", `{"cv":7,"cv_raw":8,"hv.b.dv":9,"a":6,"c_v":4}`}, {"hv.a", `{}`}} {
					path := "/api/v1/chart?chart=" + c[0]
					h.compareNow(t, path+"'s chart_variables", func(i int) string { return h.plainMember(i, path, "chart_variables") },
						healthIs(c[1]))
				}
			},
		},
		"on": {
			conf: healthVarConf,
			sc:   sc,
			play: func(t *testing.T, h *healthPair) {
				h.create(t)
				// per name the alert whose chart shares the most labels with the asked chart, the first on a tie: hv.a
				// shares two with each chart (hv.b's alert is the first), hv.c three with itself
				vars(t, h, "hv.a", append(collected, alert("hv_same", "hv.b", 2), alert("hv_one", "hv.a", 2))...)
				vars(t, h, "hv.b", append(never, alert("hv_same", "hv.b", 3), alert("hv_one", "hv.a", 2))...)
				vars(t, h, "hv.c", `"chart":"hv.c"`, alert("hv_same", "hv.c", 3), alert("hv_one", "hv.a", 2))
				traces(t, h, []healthVarTrace{
					// an alert's name: every linked alert of that name is a candidate, with its value (none: never run);
					// the same tie and the same winner by labels as `alerts`
					{"hv.a", "hv_same", "null|alarm value|hv.b|3"},
					{"hv.c", "hv_same", "null|alarm value|hv.c|3"},
					{"hv.b", "hv_one", "null|alarm value|hv.a|1"},
				})
				// with alerts on the charts a trace is still of a blank alert: `this` and `status` are 0
				traces(t, h, healthVarTraces)
			},
		},
	})
}
