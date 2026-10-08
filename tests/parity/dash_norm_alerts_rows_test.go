// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"net/http"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"
)

// dashNormAlertsCases are the alerts rows of D234 F2 and F3 by case, as their checks ask them: `api.v2-alerts`'
// `alerts`, `sets` and `off`, and `health.child` `two-hosts`'. A row that names a transition has no id here: its
// guard does not read one.
func dashNormAlertsCases() map[string][]v2Req {
	var ids [2]string
	return map[string][]v2Req{
		"alerts":    slices.Concat(alertsV2AlertRows, alertsV2TransitionRows(ids, ids)),
		"sets":      slices.Concat(alertsV2SetsRows(), alertsV2SetsScopeRows(ids), alertsV2SetsChildRows(ids), alertsV2SetsGoneRows()),
		"off":       alertsV2OffRows(),
		"two-hosts": alertsV2TwoHostsRows(),
	}
}

// dashNormAlertsNorm is a normalizer that has seen nothing, as the health runner makes one per host and side.
func dashNormAlertsNorm() *healthNorm {
	return &healthNorm{entries: map[int64]healthEntry{}, tids: map[string]int64{}, moved: map[string]int64{}}
}

// dashNormAlertsPair is a recorded row as its check compares it: its family (the alerts family with each side's
// recorded logs; the zero pair's for a row recorded without one) and the two bodies normalised by it, each in the
// seconds its own request was in flight.
type dashNormAlertsPair struct {
	fam  v2Family
	body [2][]byte
	doc  [2]Value
}

// dashNormAlertsPairOf normalises the bodies o and c as row r's family does (dashNormAlertsPair); a body that does
// not parse is a failure of the pin.
func dashNormAlertsPairOf(t *testing.T, r dashNormAlertsRow, o, c string) dashNormAlertsPair {
	t.Helper()
	norm := dashNormAlertsNorm
	recorded := func(names [2]string) func(i int) ([]byte, error) {
		return func(i int) ([]byte, error) { return []byte(dashNormAlertsLogs[names[i]]), nil }
	}
	var p dashNormAlertsPair
	switch {
	case r.logs[0] == "":
		p.fam = alertsV2Family([2]*healthNorm{}, nil)
	case r.host == "":
		p.fam = alertsV2Family([2]*healthNorm{norm(), norm()}, recorded(r.logs))
	default:
		p.fam = alertsV2Family([2]*healthNorm{norm(), norm()}, recorded(r.logs),
			alertsV2Host{name: r.host, n: [2]*healthNorm{norm(), norm()}, log: recorded(r.more)})
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

// diffs are the paths where the pair's two answers differ as compareV2 compares them: the values under the family's
// masks and unordered paths, then the layout and the strings' escapes, each named.
func (p dashNormAlertsPair) diffs() string {
	var out []string
	for _, d := range Compare(ApplyMasks(p.doc[0], p.fam.masks), ApplyMasks(p.doc[1], p.fam.masks), p.fam.unordered...) {
		out = append(out, d.Path)
	}
	if v2Layouts(p.body, p.fam.layoutByCount()) != "" {
		out = append(out, "layout")
	}
	if v2Escapes(p.body, p.doc, p.fam) != "" {
		out = append(out, "escapes")
	}
	return strings.Join(out, " ")
}

// testDashNormAlertsRows pins the alerts rows of D234 F2 and F3 (H36) on the answers and the alert logs of one
// C-against-C run (dash_norm_alerts_rows_data_test.go): each row's guard and family on its own recorded pair, what
// each guard refuses, what the render names in the long-keys, dated, MCP and two-host answers and what it leaves, the
// sets' order, the byte that is no UTF-8, and the raw rows' judge.
func testDashNormAlertsRows(t *testing.T) {
	cases := dashNormAlertsCases()
	guards, targets := map[string]func(Value) error{}, map[string]string{}
	for name, rows := range cases {
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
	// every row was recorded, and nothing else
	if got, want := slices.Sorted(maps.Keys(dashNormAlertsRows)), slices.Sorted(maps.Keys(guards)); !slices.Equal(got, want) {
		t.Fatalf("the recorded rows are %q, the checks' rows %q", got, want)
	}

	// Each recorded pair shows no difference under its family, the seconds its render named are within the bound,
	// and the row's guard takes the oracle's answer as the check hands it over: normalised.
	pairs := map[string]dashNormAlertsPair{}
	for _, key := range slices.Sorted(maps.Keys(dashNormAlertsRows)) {
		r := dashNormAlertsRows[key]
		p := dashNormAlertsPairOf(t, r, r.bodies[0], r.bodies[1])
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
		// the row still asks what was asked when its answer was recorded
		if targets[key] != r.target {
			t.Errorf("%s asks %q, its answer was recorded for %q", key, targets[key], r.target)
		}
	}

	// What each guard refuses: the answer of every other row of its case, but the rows named here, whose answers hold
	// what the guard reads (six rows that name a transition of ha_low ask one answer; a guard that reads fewer
	// members than a neighbour's takes the neighbour's answer too).
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
	if want := dashNormAlertsAccepted; !maps.EqualFunc(accepted, want, slices.Equal[[]string]) {
		for _, key := range slices.Sorted(maps.Keys(guards)) {
			if !slices.Equal(accepted[key], want[key]) {
				t.Errorf("%s: its guard also takes the answers of %q, want %q", key, accepted[key], want[key])
			}
		}
	}

	dashNormAlertsRender(t, pairs)
	dashNormAlertsPlanted(t)
	dashNormAlertsHelpers(t)
}

// dashNormAlertsAccepted are, per row, the other rows of its case whose recorded answers its guard takes too.
var dashNormAlertsAccepted = map[string][]string{
	// ha_low alone, as it is: by its status, by its name under every selector, and by a transition of its own
	"alerts/active":               {"debug-selectors", "raised", "transition", "transition-appended", "transition-first", "transition-nodash", "transition-selectors", "transition-upper"},
	"alerts/raised":               {"debug-selectors", "transition", "transition-appended", "transition-first", "transition-nodash", "transition-selectors", "transition-upper"},
	"alerts/transition":           {"debug-selectors", "raised", "transition-appended", "transition-first", "transition-nodash", "transition-selectors", "transition-upper"},
	"alerts/transition-appended":  {"debug-selectors", "raised", "transition", "transition-first", "transition-nodash", "transition-selectors", "transition-upper"},
	"alerts/transition-first":     {"debug-selectors", "raised", "transition", "transition-appended", "transition-nodash", "transition-selectors", "transition-upper"},
	"alerts/transition-nodash":    {"debug-selectors", "raised", "transition", "transition-appended", "transition-first", "transition-selectors", "transition-upper"},
	"alerts/transition-selectors": {"debug-selectors", "raised", "transition", "transition-appended", "transition-first", "transition-nodash", "transition-upper"},
	"alerts/transition-upper":     {"debug-selectors", "raised", "transition", "transition-appended", "transition-first", "transition-nodash", "transition-selectors"},
	// every alert: no status word, and a context selector that selects nothing; `v2` has them with their values
	"alerts/bogus":            {"contexts-nomatch", "v2"},
	"alerts/contexts-nomatch": {"bogus", "v2"},
	// every alert's summary without the instances: no name pattern, a lone star, no word
	"alerts/configs":        {"alert-star", "alert-wordless"},
	"alerts/alert-star":     {"alert-wordless", "configs"},
	"alerts/alert-wordless": {"alert-star", "configs"},
	// the two CLEAR alerts: by their status, and by their names
	"alerts/clear": {"by-name"},
	// no alert and the host: the guards of the rows without `instances` read no instance list
	"alerts/critical":        {"replaced"},
	"alerts/replaced":        {"critical"},
	"alerts/critical-window": {"critical", "negative", "replaced"},
	"alerts/negative":        {"critical", "critical-window", "replaced"},
	// no host under a window, with a status word and without
	"off/raised-window": {"window"},
	"off/window":        {"raised-window"},
	// `full` has the summary too
	"sets/summary": {"full"},
	// hs_sum alone on localhost: by a transition of its own, and as the one CLEAR alert left in the window
	"sets/scope-pattern":     {"gone-clear-window"},
	"sets/gone-clear-window": {"scope-pattern"},
}

// dashNormAlertsRender pins what the render wrote into the recorded oracle answers of the new forms: the long names,
// the dates, the MCP rows by position, another host's ids under its name, and the byte that is no UTF-8.
func dashNormAlertsRender(t *testing.T, pairs map[string]dashNormAlertsPair) {
	t.Helper()
	for key, wants := range map[string][]string{
		"alerts/long-debug": {`"global_id":"G",`, `"last_transition_id":"t+16",`, `"last_transition_timestamp":"WHEN",`,
			`"last_updated_timestamp":"T"`},
		"alerts/rfc3339": {`"tr_i":"t+6","tr_v":null,"tr_t":"WHEN as a date",`, `"v":null,"t":null}`,
			`"tr_i":"t+16","tr_v":70,"tr_t":"WHEN as a date",`, `"v":70,"t":"T as a date"}`},
		"alerts/mcp-instances": {`"things","t+6",null,"WHEN","8a6bb5a2-`, `"Workload",null,0]`, `"things","t+16",70,"WHEN","9eceed5c-`,
			`"root","","","",70,"T"]`},
		"alerts/mcp-rfc3339": {`"things","t+6",null,"WHEN as a date","8a6bb5a2-`, `"Workload",null,null]`,
			`"things","t+16",70,"WHEN as a date","9eceed5c-`, `"root","","","",70,"T as a date"]`},
		"sets/summary":          {`{"name":"` + alertsV2Cut + `","cr":0,`},
		"sets/two-nodes":        {`"ch":"hsetc.values",`, `"tr_i":"health-child:t+2","tr_v":10,"tr_t":"WHEN",`},
		"sets/child-transition": {`"tr_i":"health-child:t+2","tr_v":10,"tr_t":"WHEN",`},
		"two-hosts/alerts":      {`"nm":"hloc_calc",`, `"tr_i":"t+8","tr_v":70,"tr_t":"WHEN",`, `"tr_i":"health-child:t+3","tr_v":70,"tr_t":"WHEN",`},
		"two-hosts/alerts-mcp":  {`"things","t+8",70,"WHEN","`, `"things","health-child:t+3",70,"WHEN","`},
	} {
		for _, want := range wants {
			if got := string(pairs[key].body[0]); !strings.Contains(got, want) {
				t.Errorf("%s, rendered, does not hold %s: %s", key, want, got)
			}
		}
	}
	// what the render leaves: an answer of the MCP form without an instance row, and the answer of a pair without
	// health, byte for byte
	for _, key := range []string{"alerts/mcp-summary", "alerts/mcp-alone", "off/raised-window"} {
		r := dashNormAlertsRows[key]
		if got := string(pairs[key].body[0]); got != r.bodies[0] {
			t.Errorf("%s: the render changed an answer that holds no id and no clock: %s", key, got)
		}
	}
}

// dashNormAlertsSub is body with the one match of re replaced by what f makes of its groups; a pattern that matches
// no part of body, or two, is a failure of the pin.
func dashNormAlertsSub(t *testing.T, body, re string, f func(g []string) string) string {
	t.Helper()
	pattern := regexp.MustCompile(re)
	if n := len(pattern.FindAllStringIndex(body, -1)); n != 1 {
		t.Fatalf("%s matches %d parts of the recorded answer", re, n)
	}
	return pattern.ReplaceAllStringFunc(body, func(m string) string { return f(pattern.FindStringSubmatch(m)) })
}

// dashNormAlertsPlanted plants each wrong answer in the candidate's recorded body, and wants it reported at its
// paths and nowhere else; an empty want is an answer that must show no difference. A plant is a pattern with three
// groups, the second of which `to` rewrites: so the plants hold over another recorded run.
func dashNormAlertsPlanted(t *testing.T) {
	t.Helper()
	// a number moved by delta; a date moved by delta seconds, in C's shape
	moved := func(delta int64) func(string) string {
		return func(n string) string {
			v, err := strconv.ParseInt(n, 10, 64)
			if err != nil {
				t.Fatalf("%q is no number", n)
			}
			return strconv.FormatInt(v+delta, 10)
		}
	}
	dated := func(delta int64) func(string) string {
		return func(d string) string {
			v, date, ok := alertsV2Second(`"` + d + `"`)
			if !ok || !date {
				t.Fatalf("%q is no date", d)
			}
			return time.Unix(v+delta, 0).UTC().Format("2006-01-02T15:04:05Z")
		}
	}
	text := func(s string) func(string) string { return func(string) string { return s } }
	swapped := func(pair string) string {
		a, b, _ := strings.Cut(pair, ",")
		return b + "," + a
	}
	const (
		uuid = `[0-9a-f-]{36}`
		// an MCP instance row of an alert name, up to its last transition id
		mcpRow = `\["%s",[^\]]*?"things","`
		// the plain form's instance of an alert name, up to a member
		plain = `"nm":"%s"[^}]*?"%s":`
	)
	inst, lowMCP := "$.alert_instances[1]", fmt.Sprintf(mcpRow, "ha_low")
	// the second `off` after the first second (`from`) or the last (`upTo`) the row's request to the candidate was in
	// flight, as a number or as a date: what an agent that read its clock then would print for a last evaluation
	const from, upTo = 0, 1
	clock := func(r dashNormAlertsRow, edge int, off int64, date bool) string {
		v := r.flight[1][edge] + off
		if date {
			return time.Unix(v, 0).UTC().Format("2006-01-02T15:04:05Z")
		}
		return strconv.FormatInt(v, 10)
	}
	// The last evaluation, in each of the four forms: a second of the request's flight or of the healthBound seconds
	// before it is named, the second before those and the second after the flight are not.
	for form, c := range map[string]struct {
		row, re, path string
		date          bool
	}{
		"long keys": {"alerts/long-debug", `("last_updated_timestamp":)(\d+)()`, "$.alert_instances[0].last_updated_timestamp", false},
		"dates":     {"alerts/rfc3339", `("nm":"ha_low"[^}]*?"t":")([^"]*)(")`, inst + ".t", true},
		"mcp":       {"alerts/mcp-instances", `(` + lowMCP + `[^\]]*,)(\d+)(\])`, inst + "[19]", false},
		"mcp dates": {"alerts/mcp-rfc3339", `(` + lowMCP + `[^\]]*,")([^"\]]*)("\])`, inst + "[19]", true},
	} {
		r := dashNormAlertsRows[c.row]
		for when, w := range map[string]struct {
			edge int
			off  int64
			want string
		}{
			"at the request's first second":       {from, 0, ""},
			"two seconds before the request":      {from, -healthBound, ""},
			"three seconds before the request":    {from, -healthBound - 1, c.path},
			"at the request's last second":        {upTo, 0, ""},
			"a second after the request was read": {upTo, 1, c.path},
		} {
			planted := dashNormAlertsSub(t, r.bodies[1], c.re, func(g []string) string { return g[1] + clock(r, w.edge, w.off, c.date) + g[3] })
			if got := dashNormAlertsPairOf(t, r, r.bodies[0], planted).diffs(); got != w.want {
				t.Errorf("%s: evaluated %s: differences at %q, want %q", form, when, got, w.want)
			}
		}
	}

	// An alert that never had a status names its link's entry, whose time is the alert's own less the entry's
	// duration (alertsV2Entry.lastChange): in an MCP row too. The recorded sides read both clocks in one second; here
	// the candidate's entry for hm_plain is made a second after its alert.
	link := dashNormAlertsRows["alerts/mcp-instances"]
	linked := regexp.MustCompile(`\["hm_plain",[^\]]*?"things","(` + uuid + `)",null,(\d+),`).FindStringSubmatch(link.bodies[1])
	if linked == nil {
		t.Fatalf("the recorded MCP rows hold no hm_plain with a last change")
	}
	dashNormAlertsLogs["a link's entry a second late"] = dashNormAlertsSub(t, dashNormAlertsLogs[link.logs[1]],
		`("transition_id":"`+linked[1]+`","when":)(\d+)(,"duration":)0`, func(g []string) string { return g[1] + moved(1)(g[2]) + g[3] + "1" })
	defer delete(dashNormAlertsLogs, "a link's entry a second late")
	link.logs[1] = "a link's entry a second late"
	if got := dashNormAlertsPairOf(t, link, link.bodies[0], link.bodies[1]).diffs(); got != "" {
		t.Errorf("mcp: an alert made a second before its link's entry: differences at %q", got)
	}
	late := dashNormAlertsSub(t, link.bodies[1], `(\["hm_plain",[^\]]*?"things","`+uuid+`",null,)(\d+)(,)`,
		func(g []string) string { return g[1] + moved(1)(g[2]) + g[3] })
	if got, want := dashNormAlertsPairOf(t, link, link.bodies[0], late).diffs(), "$.alert_instances[0][11]"; got != want {
		t.Errorf("mcp: an alert's last change, its link's entry's time: differences at %q, want %q", got, want)
	}

	for name, c := range map[string]struct {
		row, re string
		to      func(string) string
		want    string
	}{
		// the long names
		"long keys: the last change a second late": {"alerts/long-debug", `("last_transition_timestamp":)(\d+)()`, moved(1),
			"$.alert_instances[0].last_transition_timestamp"},
		"long keys: a global id in seconds": {"alerts/long-debug", `("global_id":\d+)(\d{6})(,)`, text(""),
			"$.alert_instances[0].global_id"},
		"long keys: an id the log does not hold": {"alerts/long-debug", `("last_transition_id":")([0-9a-f]{8})(-)`, text("00000000"),
			"$.alert_instances[0].global_id $.alert_instances[0].last_transition_id $.alert_instances[0].last_transition_timestamp"},
		// the dates
		"dates: the last change a second late": {"alerts/rfc3339", `("tr_v":70,"tr_t":")([^"]*)(")`, dated(1), inst + ".tr_t"},
		"dates: a fraction":                    {"alerts/rfc3339", `("tr_v":70,"tr_t":"[^"]*)()(Z")`, text(".000"), inst + ".tr_t"},
		"dates: an offset for the Z":           {"alerts/rfc3339", `("tr_v":70,"tr_t":"[^"]*)(Z)(")`, text("+00:00"), inst + ".tr_t"},
		"dates: a number where C prints a date": {"alerts/rfc3339", `("tr_v":70,"tr_t":)("[^"]*")()`, func(d string) string {
			v, _, _ := alertsV2Second(d)
			return strconv.FormatInt(v, 10)
		}, inst + ".tr_t"},
		"dates: a null for an alert that was evaluated": {"alerts/rfc3339", `("nm":"ha_low"[^}]*?"t":)("[^"]*")(\})`, text(`null`),
			inst + ".t"},
		// the MCP rows
		"mcp: the last change a second late": {"alerts/mcp-instances", `(` + lowMCP + uuid + `",70,)(\d+)(,)`, moved(1), inst + "[11]"},
		"mcp: an id the log does not hold": {"alerts/mcp-instances", `(` + lowMCP + `)([0-9a-f]{8})(-)`, text("00000000"),
			inst + "[9] " + inst + "[11]"},
		"mcp: another host's name": {"alerts/mcp-instances", `(\["ha_low",")(parity-parent)(")`, text("parity-child"), inst + "[1]"},
		"mcp: a null id as the plain form prints it": {"alerts/mcp-instances", `(` + lowMCP[:len(lowMCP)-1] + `)("` + uuid + `")(,)`,
			text("null"), inst + "[9] " + inst + "[11]"},
		"mcp dates: the last change a second late": {"alerts/mcp-rfc3339", `(` + lowMCP + uuid + `",70,")([^"]*)(")`, dated(1),
			inst + "[11]"},
		"mcp dates: a fraction in the last item": {"alerts/mcp-rfc3339", `(` + lowMCP + `[^\]]*,"[^"\]]*)()(Z"\])`, text(".000"),
			inst + "[19]"},
		// the sets: their names' order is each side's, their names are not
		"sets: the contexts the other way round":   {"sets/summary", `("nm":"hs_two"[^}]*?"ctx":\[)([^\]]*)(\])`, swapped, ""},
		"sets: the recipients the other way round": {"sets/summary", `("nm":"hs_two"[^}]*?"to":\[)([^\]]*)(\])`, swapped, ""},
		"sets: the classes the other way round":    {"sets/summary", `("nm":"hs_two"[^}]*?"cls":\[)([^\]]*)(\])`, swapped, ""},
		"sets: the components the other way round": {"sets/summary", `("nm":"hs_two"[^}]*?"cp":\[)([^\]]*)(\])`, swapped, ""},
		"sets: the types the other way round":      {"sets/summary", `("nm":"hs_two"[^}]*?"ty":\[)([^\]]*)(\])`, swapped, ""},
		"sets: another class": {"sets/summary", `("nm":"hs_two"[^}]*?"cls":\[[^\]]*")(Latency)(")`, text("Lateness"),
			"$.alerts[1].cls[1]"},
		"sets: a type less": {"sets/summary", `("nm":"hs_two"[^}]*?"ty":\[)([^\]]*)(\])`, text(`"Type_One"`),
			"$.alerts[1].ty.length layout"},
		"sets: the hosts the other way round": {"sets/two-nodes", `("ati":0,"ni":\[)(0,1)(\])`, swapped,
			"$.alerts[0].ni[0] $.alerts[0].ni[1]"},
		"sets: one host of the two": {"sets/two-nodes", `("ati":0,"ni":\[0,1\],.*?"nd":)(2)(,)`, text("1"), "$.alerts[0].nd"},
		"mcp sets: the components the other way round": {"sets/mcp-summary", `(\["hs_two","",\[[^\]]*\],\[[^\]]*\],\[)([^\]]*)(\])`,
			swapped, ""},
		"mcp sets: another context": {"sets/mcp-summary", `(\["hs_two","",\[[^\]]*")(hset\.ctx\.b)(")`, text("hset.ctx.c"),
			"$.all_alerts[1][2][1]"},
		"mcp sets: one name as a list": {"sets/mcp-summary", `(\["hs_tpl","[^"]*",)("hset\.ctx\.a")(,)`, text(`["hset.ctx.a"]`),
			"$.all_alerts[0][2] layout"},
		// the module's 127 bytes: the last one is no UTF-8 (a pattern reads it as one character), and the render writes
		// it out in a text no agent's own text is taken for
		"bytes: U+FFFD for the half character": {"sets/summary", `("name":"m{126})(.)(")`, text("\ufffd"), "$.alerts_by_module[1].name"},
		"bytes: the module cut at a character": {"sets/summary", `("name":"m{126})(.)(")`, text(""), "$.alerts_by_module[1].name"},
		"bytes: the whole character":           {"sets/summary", `("name":"m{126})(.)(")`, text("\u00e9"), "$.alerts_by_module[1].name"},
		"bytes: the byte spelled out":          {"sets/summary", `("name":"m{126})(.)(")`, text(`\\xc3`), "$.alerts_by_module[1].name escapes"},
		"bytes: the render's own text for it":  {"sets/summary", `("name":"m{126})(.)(")`, text(string(alertsV2Marker) + "c3"), "$.alerts_by_module[1].name"},
		// the same text with the marker as a JSON escape parses to the oracle's: its escape tells it apart
		"bytes: the render's text, its first character as an escape": {"sets/summary", `("name":"m{126})(.)(")`, text(`\uf8ffc3`), "escapes"},
		// two hosts
		"two hosts: the child's last change a second late": {"two-hosts/alerts", `(` + fmt.Sprintf(plain, "hch_calc", "tr_t") + `)(\d+)(,)`,
			moved(1), inst + ".tr_t"},
		"two hosts: the child's instance under localhost's index": {"two-hosts/alerts", `(\{"ati":1,"ni":)(1)(,)`, text("0"),
			inst + ".ni"},
		"two hosts: the child's index after it was listed": {"two-hosts/alerts", `(\{"ati":1,"ni":)(1)(,)`, text("2"), inst + ".ni"},
		"two hosts: the child's row under localhost's name": {"two-hosts/alerts-mcp", `(\["hch_calc",")(health-child)(")`,
			text("parity-parent"), inst + "[1]"},
		"two hosts: the child's row's last change a second late": {"two-hosts/alerts-mcp",
			`(` + fmt.Sprintf(mcpRow, "hch_calc") + uuid + `",70,)(\d+)(,)`, moved(1), inst + "[11]"},
		// a pair without health: a host listed under a window
		"off: the host listed": {"off/raised-window", `("nodes":\[)()(\])`,
			text(`{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}`), "$.nodes.length layout"},
	} {
		r, ok := dashNormAlertsRows[c.row]
		if !ok {
			t.Fatalf("%s: no recorded row %s", name, c.row)
		}
		planted := dashNormAlertsSub(t, r.bodies[1], c.re, func(g []string) string { return g[1] + c.to(g[2]) + g[3] })
		if got := dashNormAlertsPairOf(t, r, r.bodies[0], planted).diffs(); got != c.want {
			t.Errorf("%s: differences at %q, want %q", name, got, c.want)
		}
	}

	// an id the log does not hold is left as the agent wrote it, in the long form and in an MCP row
	for row, re := range map[string]string{"alerts/long-debug": `("last_transition_id":")([0-9a-f]{8})(-)`,
		"alerts/mcp-instances": `(` + lowMCP + `)([0-9a-f]{8})(-)`} {
		r := dashNormAlertsRows[row]
		planted := dashNormAlertsSub(t, r.bodies[1], re, func(g []string) string { return g[1] + "00000000" + g[3] })
		if got := string(dashNormAlertsPairOf(t, r, r.bodies[0], planted).body[1]); !strings.Contains(got, `"00000000-`) ||
			strings.Contains(got, `"t?"`) {
			t.Errorf("%s: an id the log does not hold, rendered: %s", row, got)
		}
	}

	// Without the child's alert log the child's alert is no entry of the side's: its ids stay as each agent wrote
	// them, and the recorded pair differs there, and nowhere but in the child's item (its last change stays as written
	// too, and differs when the two sides' children changed in two seconds; its last evaluation is read against the
	// flight).
	for key, ids := range map[string][]string{
		"two-hosts/alerts":     {inst + ".gi", inst + ".tr_i"},
		"two-hosts/alerts-mcp": {inst + "[9]"},
	} {
		r := dashNormAlertsRows[key]
		r.host = ""
		got := strings.Fields(dashNormAlertsPairOf(t, r, r.bodies[0], r.bodies[1]).diffs())
		for _, id := range ids {
			if !slices.Contains(got, id) {
				t.Errorf("%s without the child's log: differences at %q, none at %s", key, got, id)
			}
		}
		if slices.ContainsFunc(got, func(path string) bool { return !strings.HasPrefix(path, inst) }) {
			t.Errorf("%s without the child's log: differences at %q, outside the child's item", key, got)
		}
	}
}

// dashNormAlertsHelpers pins the pieces of the rows that read no recorded pair: the second a clock holds, the bytes
// that are no UTF-8, the guard's views, another host's log, a transition row's targets, the family's shape and the
// raw rows' judge.
func dashNormAlertsHelpers(t *testing.T) {
	t.Helper()
	// a second as written: a number, or a date in C's one shape
	for text, want := range map[string][3]any{
		`1791462312`:                  {int64(1791462312), false, true},
		`"2026-10-08T12:25:12Z"`:      {int64(1791462312), true, true},
		`"2026-10-08T12:25:12.000Z"`:  {int64(0), false, false},
		`"2026-10-08T12:25:12+00:00"`: {int64(0), false, false},
		`"2026-10-08 12:25:12Z"`:      {int64(0), false, false},
		`"2026-10-08T12:25:12"`:       {int64(0), false, false},
		`"1791462312"`:                {int64(0), false, false},
		`null`:                        {int64(0), false, false},
	} {
		if v, date, ok := alertsV2Second(text); v != want[0] || date != want[1] || ok != want[2] {
			t.Errorf("the second of %s: %d, a date %t, a second %t, want %v", text, v, date, ok, want)
		}
	}
	// bytes that are no UTF-8 are written out, everything else is left
	for in, want := range map[string]string{
		`{"a":"m` + "\xc3" + `"}`:         `{"a":"m` + "\uf8ffc3" + `"}`,
		`{"a":"` + "\x80\xff" + `z"}`:     `{"a":"` + "\uf8ff80\uf8ffff" + `z"}`,
		`{"a":"m` + "\u00e9\ufffd" + `"}`: `{"a":"m` + "\u00e9\ufffd" + `"}`,
		`{"a":"plain"}`:                   `{"a":"plain"}`,
		// a U+FFFD the agent wrote stays, beside a byte that is none
		`{"a":"` + "\xc3 \ufffd" + `"}`: `{"a":"` + "\uf8ffc3 \ufffd" + `"}`,
		// the marker an agent wrote itself is doubled, beside a byte and alone: nothing an agent writes reads as a byte
		`{"a":"` + "\uf8ffc3 \xc3" + `"}`: `{"a":"` + "\uf8ff\uf8ffc3 \uf8ffc3" + `"}`,
		`{"a":"` + "\uf8ffc3" + `"}`:      `{"a":"` + "\uf8ff\uf8ffc3" + `"}`,
	} {
		got := string(alertsV2Bytes([]byte(in)))
		if got != want {
			t.Errorf("the bytes of %q: %q, want %q", in, got, want)
		}
		if _, err := ParseJSON([]byte(got)); err != nil {
			t.Errorf("the bytes of %q do not parse: %v", in, err)
		}
	}
	// the guard's views of a member: an item of a row by its index, a list as a set, a member an item may lack
	doc, err := ParseJSON([]byte(`{"rows":[["b",["y","x"],null,"one"],["a",["x","y"],null,["p"]]],` +
		`"items":[{"name":"n","available":1,"in":{"k":2}},{"name":"m","in":{"k":3}}],"none":[],"null":null,"count":0,"text":""}`))
	if err != nil {
		t.Fatal(err)
	}
	for name, c := range map[string]struct {
		rows alertsV2Rows
		bad  bool
	}{
		"a row's items by index, a list as a set": {alertsV2Rows{member: "rows", keys: []string{"[0]", "[1]{}", "[2]", "[3]{}"},
			items: []string{"b {x,y} null one", "a {x,y} null {p}"}}, false},
		"a list in its order": {alertsV2Rows{member: "rows", keys: []string{"[0]", "[1]"},
			items: []string{`b ["y","x"]`, `a ["x","y"]`}}, false},
		"a set of other names":       {alertsV2Rows{member: "rows", keys: []string{"[1]{}"}, items: []string{"{x,z}", "{x,y}"}}, true},
		"an item past the row's end": {alertsV2Rows{member: "rows", keys: []string{"[4]"}, items: []string{"", ""}}, true},
		"a member an item lacks, optional": {alertsV2Rows{member: "items", keys: []string{"name", "available?", "in.k"},
			items: []string{"n 1 2", "m - 3"}}, false},
		"a member an item lacks":           {alertsV2Rows{member: "items", keys: []string{"name", "available"}, items: []string{"n 1", "m -"}}, true},
		"an optional member that is there": {alertsV2Rows{member: "items", keys: []string{"name?"}, items: []string{"-", "-"}}, true},
		"the rows in another order":        {alertsV2Rows{member: "rows", keys: []string{"[0]"}, items: []string{"a", "b"}}, true},
		"the rows as a sorted set":         {alertsV2Rows{member: "rows", keys: []string{"[0]"}, items: []string{"a", "b"}, sorted: true}, false},
		"a member that must be absent":     {alertsV2Rows{member: "rows"}, true},
		"an absent member":                 {alertsV2Rows{member: "nothing"}, false},
		"an empty list wanted":             {alertsV2Rows{member: "rows", keys: []string{"[0]"}, items: []string{}}, true},
		"an empty list":                    {alertsV2Rows{member: "none", keys: []string{"[0]"}, items: []string{}}, false},
		"null for an empty list":           {alertsV2Rows{member: "null", keys: []string{"[0]"}, items: []string{}}, true},
		"a number for an empty list":       {alertsV2Rows{member: "count", keys: []string{"[0]"}, items: []string{}}, true},
		"a text for an empty list":         {alertsV2Rows{member: "text", keys: []string{"[0]"}, items: []string{}}, true},
	} {
		if err := alertsV2Guard(c.rows)(doc); (err != nil) != c.bad {
			t.Errorf("the guard, %s: %v", name, err)
		}
	}
	if got := alertsV2Nodes(); got.items == nil || len(got.items) != 0 || got.member != "nodes" {
		t.Errorf("the guard's view of no node wants %v of %s", got.items, got.member)
	}
	// another host's log joins the side's: its entries named by its host, an id of both an error
	norm := dashNormAlertsNorm
	const (
		own    = `[{"unique_id":101,"transition_id":"11111111-1111-4111-8111-111111111111","when":5,"status":"CLEAR","old_status":"UNINITIALIZED"}]`
		theirs = `[{"unique_id":204,"transition_id":"22222222-2222-4222-8222-222222222222","when":7,"status":"CLEAR","old_status":"UNINITIALIZED"},` +
			`{"unique_id":203,"transition_id":"33333333-3333-4333-8333-333333333333","when":6,"status":"UNINITIALIZED","old_status":"REMOVED"}]`
	)
	n, cn := norm(), norm()
	log, err := alertsV2LogOf([]byte(own))
	if err != nil {
		t.Fatal(err)
	}
	n.observe(own)
	if err := log.join("kid", cn, []byte(theirs)); err != nil {
		t.Fatal(err)
	}
	for id, want := range map[string]string{"11111111-1111-4111-8111-111111111111": "t+1",
		"22222222-2222-4222-8222-222222222222": "kid:t+2", "33333333-3333-4333-8333-333333333333": "kid:t+1"} {
		if got := log[id].named(n); got != want {
			t.Errorf("the entry of %s is named %q, want %q", id, got, want)
		}
	}
	if log["22222222-2222-4222-8222-222222222222"].When != 7 || len(log) != 3 {
		t.Errorf("the joined log holds %d entries, the child's of the second %d", len(log), log["22222222-2222-4222-8222-222222222222"].When)
	}
	if err := log.join("kid", cn, []byte(own)); err == nil || !strings.Contains(err.Error(), "in the agent's own log and in kid's") {
		t.Errorf("a transition id of two logs: %v", err)
	}
	if err := log.join("kid", cn, []byte(`{`)); err == nil {
		t.Errorf("a log that is no JSON joined")
	}
	// a family whose other host's log cannot be read leaves the body unparsable, with the reason
	broken := alertsV2Family([2]*healthNorm{norm(), norm()}, func(int) ([]byte, error) { return []byte(own), nil },
		alertsV2Host{name: "kid", n: [2]*healthNorm{norm(), norm()}, log: func(int) ([]byte, error) { return nil, fmt.Errorf("no log") }})
	if got := string(broken.render(0, [2]int64{}, []byte(`{"gi":1}`))); got != "the side's alert log: no log\n"+`{"gi":1}` {
		t.Errorf("a side without its other host's log: %q", got)
	}
	// a side whose own log cannot be read says so, whatever its other host's log
	broken = alertsV2Family([2]*healthNorm{norm(), norm()}, func(int) ([]byte, error) { return nil, fmt.Errorf("no own log") },
		alertsV2Host{name: "kid", n: [2]*healthNorm{norm(), norm()}, log: func(int) ([]byte, error) { return []byte(theirs), nil }})
	if got := string(broken.render(0, [2]int64{}, []byte(`{"gi":1}`))); got != "the side's alert log: no own log\n"+`{"gi":1}` {
		t.Errorf("a side without its own log: %q", got)
	}
	// the family: the five sets unordered, by their short and long keys and in the MCP rows; a flat layout
	fam := alertsV2Family([2]*healthNorm{norm(), norm()}, func(int) ([]byte, error) { return []byte(own), nil })
	if !slices.Equal(fam.unordered, alertsV2Sets) || len(alertsV2Sets) != 15 || !fam.flat || fam.layoutByCount() {
		t.Errorf("the alerts family leaves %q unordered, flat %t", fam.unordered, fam.flat)
	}
	for _, key := range []string{"ctx", "cls", "cp", "ty", "to", "contexts", "classifications", "components", "types", "recipients"} {
		o, _ := ParseJSON([]byte(`{"alerts":[{"ni":[0,1],"` + key + `":["a","b"]}]}`))
		c, _ := ParseJSON([]byte(`{"alerts":[{"ni":[0,1],"` + key + `":["b","a"]}]}`))
		if d := Compare(o, c, fam.unordered...); len(d) != 0 {
			t.Errorf("the alerts family compares the names of `%s` in their order: %v", key, d)
		}
	}
	for at := 0; at < 14; at++ {
		row := func(pair string) Value {
			items := slices.Repeat([]string{"0"}, 14)
			items[at] = pair
			v, _ := ParseJSON([]byte(`{"all_alerts":[[` + strings.Join(items, ",") + `]]}`))
			return v
		}
		d := Compare(row(`["a","b"]`), row(`["b","a"]`), fam.unordered...)
		if set := at >= 2 && at <= 6; set != (len(d) == 0) {
			t.Errorf("the alerts family, item %d of an MCP summary row with its two names swapped: %v", at, d)
		}
	}
	for _, body := range []string{`{"alerts":[{"ni":%s}]}`, `{"alert_instances":[{"ctx":%s}]}`, `{"nodes":%s}`, `{"alerts_by_type":%s}`} {
		o, _ := ParseJSON([]byte(fmt.Sprintf(body, `["a","b"]`)))
		c, _ := ParseJSON([]byte(fmt.Sprintf(body, `["b","a"]`)))
		if d := Compare(o, c, fam.unordered...); len(d) == 0 {
			t.Errorf("the alerts family compares %s as a set", body)
		}
	}
	// the newest change of an alert of a log: by its name and both statuses, the last one that matches
	entries := []healthEntry{
		{Name: "a", OldStatus: "UNINITIALIZED", Status: "CLEAR", Tid: "first"}, {Name: "b", OldStatus: "CLEAR", Status: "WARNING", Tid: "other"},
		{Name: "a", OldStatus: "CLEAR", Status: "WARNING", Tid: "raised"}, {Name: "a", OldStatus: "UNINITIALIZED", Status: "CLEAR", Tid: "again"},
		{Name: "a", OldStatus: "CLEAR", Status: "CRITICAL", Tid: "worse"}, {Name: "a", OldStatus: "CRITICAL", Status: "CLEAR", Tid: "back"},
	}
	for want, c := range map[string][3]string{"again": {"a", "UNINITIALIZED", "CLEAR"}, "raised": {"a", "CLEAR", "WARNING"},
		"other": {"b", "CLEAR", "WARNING"}, "back": {"a", "CRITICAL", "CLEAR"}, "": {"b", "UNINITIALIZED", "CLEAR"}} {
		if got := alertsV2NewestChange(entries, c[0], c[1], c[2]); got != want {
			t.Errorf("the newest change of %s from %s to %s is %q, want %q", c[0], c[1], c[2], got, want)
		}
	}
	// the rows of the `alerts` case that name a transition: which id each asks, and how it spells it
	last, first := [2]string{"aaaaaaaa-1111-4111-8111-00000000000a", "bbbbbbbb-1111-4111-8111-00000000000b"},
		[2]string{"cccccccc-1111-4111-8111-00000000000c", "dddddddd-1111-4111-8111-00000000000d"}
	spelled := map[string][2]string{}
	for _, row := range alertsV2TransitionRows(last, first) {
		var ids [2]string
		for i := range ids {
			_, query, _ := strings.Cut(row.targetOf(i), "?transition=")
			ids[i], _, _ = strings.Cut(query, "&")
		}
		spelled[row.name] = ids
	}
	if want := map[string][2]string{
		"transition": last, "transition-first": first, "transition-selectors": last, "transition-bare": last, "transition-mcp": last,
		"transition-critical": last,
		"transition-nodash":   {"aaaaaaaa1111411181110000000000" + "0a", "bbbbbbbb1111411181110000000000" + "0b"},
		"transition-upper":    {"AAAAAAAA-1111-4111-8111-00000000000A", "BBBBBBBB-1111-4111-8111-00000000000B"},
		"transition-appended": {last[0] + "zz", last[1] + "zz"},
	}; !maps.Equal(spelled, want) {
		t.Errorf("the transition rows ask the ids %v, want %v", spelled, want)
	}
	// a row that names a transition: each side its own id, in the query's place
	req := alertsV2TransitionRow("r", [2]string{"A", "B"}, "&x=1", nil)
	if req.targetOf(0) != alertsV2Route+"?transition=A&x=1" || req.targetOf(1) != alertsV2Route+"?transition=B&x=1" || req.status != "200" {
		t.Errorf("a transition row asks %q and %q, wants %s", req.targetOf(0), req.targetOf(1), req.status)
	}

	// The raw rows' requests, as they are sent: the first two as every row's (v2Request), the third on a connection
	// it asks to keep, to a client that takes gzip.
	const unknown = "GET /api/v3/alerts?transition=11111111-2222-4333-8444-555555555555 HTTP/1.1\r\nHost: localhost\r\n"
	for _, row := range alertsV2NotFoundRows {
		want := map[string]string{
			"transition-unknown": unknown + "Connection: close\r\n\r\n",
			"transition-text":    "GET /api/v3/alerts?transition=x HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
			"transition-gzip":    unknown + "Connection: keep-alive\r\nAccept-Encoding: gzip\r\n\r\n",
		}[row.name]
		if string(row.request) != want {
			t.Errorf("the raw row %s sends %q, want %q", row.name, row.request, want)
		}
	}
	// The round of a raw row, asked of two stub agents that write C's 404 with the Date of the second they answer in:
	// no problem between two of them; a candidate with one thing more or other is reported, and so is an oracle that
	// does not answer what the row wants.
	notFound := func(status, more, body string) func() string {
		return func() string {
			now := time.Now().UTC().Format(http.TimeFormat)
			return "HTTP/1.1 " + status + "\r\nConnection: close\r\nServer: stub\r\nDate: " + now +
				"\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-cache, no-store, must-revalidate\r\n" +
				"Pragma: no-cache\r\nExpires: " + now + "\r\n" + more + "X-Transaction-ID: 0123456789abcdef0123456789abcdef\r\n\r\n" + body
		}
	}
	c404 := notFound("404 Not Found", "", "")
	for name, c := range map[string]struct {
		o, c func() string
		want string
	}{
		"C's answers":                  {c404, c404, ""},
		"a candidate with a body":      {c404, notFound("404 Not Found", "", "Not Found"), "answers differ"},
		"a candidate with a length":    {c404, notFound("404 Not Found", "Content-Length: 0\r\n", ""), "answers differ"},
		"a candidate that answers 200": {c404, notFound("200 OK", "", ""), "answers differ"},
		"an oracle with a body":        {notFound("404 Not Found", "", "Not Found"), c404, "oracle: answered"},
		"an oracle that answers 200":   {notFound("200 OK", "", ""), notFound("200 OK", "", ""), "oracle: answered"},
	} {
		pair := &Pair{Oracle: dashNormStub(t, c.o), Candidate: dashNormStub(t, c.c)}
		got := strings.Join(alertsV2RawRound(pair, alertsV2NotFoundRows[0]), "\n")
		if (c.want == "") != (got == "") || !strings.Contains(got, c.want) {
			t.Errorf("the raw round, %s: problems %q, want %q", name, got, c.want)
		}
	}

	// The raw rows: each recorded answer, masked for the second its own Date says, is what its row wants, and the
	// two sides' are equal; then the answers a row refuses.
	raws := map[string]alertsV2Raw{}
	for _, row := range alertsV2NotFoundRows {
		raws["alerts/"+row.name] = row
	}
	if got, want := slices.Sorted(maps.Keys(dashNormAlertsRaw)), slices.Sorted(maps.Keys(raws)); !slices.Equal(got, want) {
		t.Fatalf("the recorded raw rows are %q, the check's %q", got, want)
	}
	masked := func(raw string) []byte {
		var flight [2]int64
		for _, l := range strings.Split(raw, "\r\n") {
			if v, ok := strings.CutPrefix(l, "Date: "); ok {
				d, _ := headDate(v)
				flight = [2]int64{d, d}
			}
		}
		return maskAnswer([]byte(raw), flight)
	}
	for key, row := range raws {
		answers := dashNormAlertsRaw[key]
		o, c := masked(answers[0]), masked(answers[1])
		if err := alertsV2RawJudge(row, o); err != nil {
			t.Errorf("%s: the judge refuses C's answer: %v", key, err)
		}
		if string(o) != string(c) {
			t.Errorf("%s: the recorded answers differ once masked:\n%s", key, firstDifference(o, c))
		}
		for name, wrong := range map[string]string{
			"a 200":                      strings.Replace(string(o), "HTTP/1.1 404 Not Found\r\n", "HTTP/1.1 200 OK\r\n", 1),
			"a length":                   strings.Replace(string(o), "\r\nX-Transaction-ID:", "\r\nContent-Length: 0\r\nX-Transaction-ID:", 1),
			"a body":                     string(o) + "Not Found",
			"a body that ends as a head": string(o) + "Not Found\r\n\r\n",
			"a last chunk":               string(o) + "0\r\n\r\n",
			"a JSON type":                strings.Replace(string(o), "text/plain", "application/json", 1),
			"a cacheable answer":         strings.Replace(string(o), "Expires: Date+0", "Expires: Date+86400", 1),
			"a public answer":            strings.Replace(string(o), "no-cache, no-store, must-revalidate", "public", 1),
			"an open connection":         string(o) + "<timeout>",
		} {
			if wrong == string(o) {
				t.Fatalf("%s: %s is C's answer: the plant did not apply", key, name)
			}
			if err := alertsV2RawJudge(row, []byte(wrong)); err == nil {
				t.Errorf("%s: the judge takes %s", key, name)
			}
		}
	}
	for _, key := range []string{"alerts/transition-unknown", "alerts/transition-text"} {
		o := string(masked(dashNormAlertsRaw[key][0]))
		for name, wrong := range map[string]string{
			"an encoding":                         strings.Replace(o, "\r\nX-Transaction-ID:", "\r\nContent-Encoding: gzip\r\nX-Transaction-ID:", 1),
			"a chunked answer":                    strings.Replace(o, "\r\nX-Transaction-ID:", "\r\nTransfer-Encoding: chunked\r\nX-Transaction-ID:", 1),
			"an answer that keeps the connection": strings.Replace(o, "Connection: close", "Connection: keep-alive", 1),
		} {
			if wrong == o {
				t.Fatalf("%s: %s is C's answer: the plant did not apply", key, name)
			}
			if err := alertsV2RawJudge(raws[key], []byte(wrong)); err == nil {
				t.Errorf("%s: the judge takes %s", key, name)
			}
		}
	}
	gzip := raws["alerts/transition-gzip"]
	o := masked(dashNormAlertsRaw["alerts/transition-gzip"][0])
	for name, wrong := range map[string]string{
		"an answer without the encoding":      strings.Replace(string(o), "\r\nContent-Encoding: gzip", "", 1),
		"an answer that is not chunked":       strings.Replace(string(o), "\r\nTransfer-Encoding: chunked", "", 1),
		"an answer that says it closes":       strings.Replace(string(o), "Connection: keep-alive", "Connection: close", 1),
		"the answer of a client without gzip": string(masked(dashNormAlertsRaw["alerts/transition-unknown"][0])),
	} {
		if err := alertsV2RawJudge(gzip, []byte(wrong)); err == nil {
			t.Errorf("the gzip row's judge takes %s", name)
		}
	}
	if err := alertsV2RawJudge(raws["alerts/transition-unknown"], o); err == nil {
		t.Errorf("the plain row's judge takes the gzip answer")
	}
}
