// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"slices"
	"strconv"
	"strings"
	"testing"
)

// dashNormClosersLCases are SA-L's closer rows by case, as their checks ask them (alerts_closers_l_test.go):
// `api.v2-alerts`' `transitions` (the alerts rows at CRITICAL, the echo) and `states`, `health.child`'s `two-hosts`
// (the node selectors, the transitions of two hosts) and `half-off`. A row that names a transition has no id here: its
// guard does not read one.
func dashNormClosersLCases() map[string][]v2Req {
	var ids [2]string
	return map[string][]v2Req{
		"transitions": slices.Concat(alertsV2TransitionsCriticalRows(), alertsV2TransitionsEchoRows(ids)),
		"states":      slices.Concat(alertsV2StatesRows(), alertsV2StatesOneRows(map[string][2]string{})),
		"two-hosts":   slices.Concat(alertsV2TwoHostsNodeRows(), alertsV2TwoHostsTransitionRows(ids)),
		"half-off":    alertsV2HalfOffRows(),
	}
}

// dashNormClosersLPairOf normalises the bodies o and c as the recorded row r's family does: the alerts family with
// each side's recorded log and, where the row has one, the child's (alertsV2Host), each body in the seconds its own
// request was in flight (dashNormAlertsPair).
func dashNormClosersLPairOf(t *testing.T, r dashNormClosersLRow, o, c string) dashNormAlertsPair {
	t.Helper()
	recorded := func(names [2]string) func(i int) ([]byte, error) {
		return func(i int) ([]byte, error) { return []byte(dashNormClosersLLogs[names[i]]), nil }
	}
	norm := dashNormAlertsNorm
	var p dashNormAlertsPair
	if r.more[0] == "" {
		p.fam = alertsV2Family([2]*healthNorm{norm(), norm()}, recorded(r.logs))
	} else {
		p.fam = alertsV2Family([2]*healthNorm{norm(), norm()}, recorded(r.logs),
			alertsV2Host{name: healthChild.Hostname, n: [2]*healthNorm{norm(), norm()}, log: recorded(r.more)})
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

// testDashNormClosersLRows pins SA-L's rows on the answers and the alert logs of one C-against-C run
// (dash_norm_closers_l_data_test.go): each row's guard and family on its own recorded pair, what each guard refuses,
// what the render names (the echo of a transition, another host's ids, the dates and marks of the new texts), and each
// named wrong answer planted in the candidate's body.
func testDashNormClosersLRows(t *testing.T) {
	guards, targets := map[string]func(Value) error{}, map[string]string{}
	for name, rows := range dashNormClosersLCases() {
		for _, req := range rows {
			key := name + "/" + req.name
			if req.guard == nil || req.status != "200" {
				t.Errorf("%s: a row without a guard, or one that wants another status than 200", key)
			}
			if _, twice := guards[key]; twice {
				t.Errorf("%s: two rows of one name", key)
			}
			guards[key], targets[key] = req.guard, req.target
		}
	}
	// every row was recorded, and nothing else
	if got, want := slices.Sorted(maps.Keys(dashNormClosersLRows)), slices.Sorted(maps.Keys(guards)); !slices.Equal(got, want) {
		t.Fatalf("the recorded rows are %q, the checks' rows %q", got, want)
	}

	// Each recorded pair shows no difference under its family, the seconds its render named are within the bound,
	// and the row's guard takes the oracle's answer as the check hands it over: normalised.
	pairs := map[string]dashNormAlertsPair{}
	for _, key := range slices.Sorted(maps.Keys(dashNormClosersLRows)) {
		r := dashNormClosersLRows[key]
		p := dashNormClosersLPairOf(t, r, r.bodies[0], r.bodies[1])
		pairs[key] = p
		if got := p.diffs(); got != "" {
			t.Errorf("%s: the recorded pair differs at %q", key, got)
		}
		p.fam.check(t, key, p.doc[0], p.doc[1])
		if err := guards[key](p.doc[0]); err != nil {
			t.Errorf("%s: its guard refuses C's answer: %v", key, err)
		}
		// the row still asks what was asked when its answer was recorded
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
	if want := dashNormClosersLAccepted; !maps.EqualFunc(accepted, want, slices.Equal[[]string]) {
		for _, key := range slices.Sorted(maps.Keys(guards)) {
			if !slices.Equal(accepted[key], want[key]) {
				t.Errorf("%s: its guard also takes the answers of %q, want %q", key, accepted[key], want[key])
			}
		}
	}

	dashNormClosersLRender(t, pairs)
	dashNormClosersLPlanted(t)
	dashNormClosersLRefused(t, guards)
}

// dashNormClosersLItems are the items of the array that follows `"<member>":[` in a JSON text, each as it is written
// (strings read with their escapes, so a bracket inside one is no item's end), with the text before the first item and
// after the last.
func dashNormClosersLItems(t *testing.T, body, member string) (head string, items []string, tail string) {
	t.Helper()
	start := strings.Index(body, `"`+member+`":[`)
	if start < 0 {
		t.Fatalf("no %s in %s", member, body)
	}
	i := start + len(member) + 4
	head = body[:i]
	depth, from, quoted := 0, i, false
	for ; i < len(body); i++ {
		c := body[i]
		switch {
		case quoted && c == '\\':
			i++
		case c == '"':
			quoted = !quoted
		case quoted:
		case c == '{' || c == '[':
			if depth == 0 {
				from = i
			}
			depth++
		case c == '}' || c == ']':
			if depth == 0 {
				return head, items, body[i:]
			}
			if depth--; depth == 0 {
				items = append(items, body[from:i+1])
			}
		}
	}
	t.Fatalf("the array %s has no end in %s", member, body)
	return
}

// dashNormClosersLSwap is body with the items i and j of its array `member` the other way round.
func dashNormClosersLSwap(t *testing.T, body, member string, i, j int) string {
	t.Helper()
	head, items, tail := dashNormClosersLItems(t, body, member)
	if i >= len(items) || j >= len(items) {
		t.Fatalf("%s has %d items, not %d and %d", member, len(items), i, j)
	}
	items[i], items[j] = items[j], items[i]
	return head + strings.Join(items, ",") + tail
}

// dashNormClosersLAccepted are, per row, the other rows of its case whose recorded answers its guard takes too: the
// rows that ask one answer by two requests (no other guard takes another row's answer).
var dashNormClosersLAccepted = map[string][]string{
	// localhost's alert and both hosts without a window, with a status word and without; localhost alone under one
	"half-off/plain": {"raised"}, "half-off/raised": {"plain"},
	"half-off/raised-window": {"window"}, "half-off/window": {"raised-window"},
	// the three CRITICAL alerts by `critical` and `raised`, under a window and without; `critical` without one also has
	// their values, which its guard reads
	"transitions/alerts-critical-window": {"alerts-critical", "alerts-raised"},
	"transitions/alerts-raised":          {"alerts-critical", "alerts-critical-window"},
	// no WARNING alert, the host listed: under a window and without (the window's guard reads no instance list)
	"transitions/alerts-warning-window": {"alerts-warning"},
	// one host by its hostname or its GUID; localhost by its scope or by the child's negative word
	"two-hosts/nodes-child": {"nodes-child-guid"}, "two-hosts/nodes-child-guid": {"nodes-child"},
	"two-hosts/nodes-not-child": {"scope-nodes-parent"}, "two-hosts/scope-nodes-parent": {"nodes-not-child"},
}

// dashNormClosersLRender pins what the render wrote into the recorded oracle answers: the echo of a transition id as
// the alias of the answering host's entry (localhost's, the child's), another host's ids under its name and
// localhost's under none, the MCP form over two hosts named by each change, the delay's marks, and the half
// characters of the cut texts written out.
func dashNormClosersLRender(t *testing.T, pairs map[string]dashNormAlertsPair) {
	t.Helper()
	for key, wants := range map[string][]string{
		"transitions/echo-alerts":          {`"transition":"t+`, `"tr_i":"t+`},
		"transitions/echo-transitions":     {`"transition":"t+`, `"transition_id":"t+`},
		"two-hosts/transition-child-debug": {`"transition":"health-child:t+`, `"transition_id":"health-child:t+`},
		"two-hosts/transitions":            {`"alert":"hch_calc","transition_id":"health-child:t+`, `"alert":"hloc_calc","transition_id":"t+`},
		"two-hosts/transitions-mcp": {`{"gi":"G","alert":"hch_calc","config_hash_id":`, `{"gi":"G","alert":"hloc_calc","config_hash_id":`,
			`"type":null,"when":"WHEN",`},
		"states/delay": {`"notification":{"when":"EXEC_RUN","delay":10,"delay_up_to_time":"DELAY_UP_TO",`},
		"states/long": {`"summary":"` + strings.Repeat("s", 510) + string(alertsV2Marker) + `c3","units":"` + strings.Repeat("u", 46) +
			string(alertsV2Marker) + `c3",`},
	} {
		for _, want := range wants {
			if got := string(pairs[key].body[0]); !strings.Contains(got, want) {
				t.Errorf("%s, rendered, does not hold %s: %s", key, want, got)
			}
		}
	}

	// the echo's alias on hand-made bodies: an id of the log as the log prints it, with or without a blank after the
	// colon, the other host's under its name; any other text, a null and the transition's own key stay
	log := alertsV2Log{"id-a": {Tid: "id-a", as: "t+1"}, "id-b": {Tid: "id-b", as: healthChild.Hostname + ":t+2"}}
	for body, want := range map[string]string{
		`{"transition":"id-a"}`:         `{"transition":"t+1"}`,
		`{"transition": "id-a"}`:        `{"transition": "t+1"}`,
		`{"transition":"id-b"}`:         `{"transition":"health-child:t+2"}`,
		`{"transition":"ID-A"}`:         `{"transition":"ID-A"}`,
		`{"transition":"id-a "}`:        `{"transition":"id-a "}`,
		`{"transition":null}`:           `{"transition":null}`,
		`{"transition_id":"id-a"}`:      `{"transition_id":"id-a"}`,
		`{"xtransition":"id-a"}`:        `{"xtransition":"id-a"}`,
		`{"transition":"id-c","a":"x"}`: `{"transition":"id-c","a":"x"}`,
	} {
		if got := string(alertsV2EchoAlias(dashNormAlertsNorm(), log, []byte(body))); got != want {
			t.Errorf("the echo of %s reads %s, want %s", body, got, want)
		}
	}

	// the two facts on hand-made answers: the echo is an alias of the right host and the transition's own id; each id
	// is its own host's alias
	for body, refused := range map[string]bool{
		`{"request":{"selectors":{"alerts":{"transition":"t+1"}}},"transitions":[{"transition_id":"t+1"}]}`: false,
		`{"request":{"selectors":{"alerts":{"transition":"t+1"}}}}`:                                         false,
		`{"request":{"selectors":{"alerts":{"transition":"t+1"}}},"transitions":[{"transition_id":"t+2"}]}`: true,
		`{"request":{"selectors":{"alerts":{"transition":"t?"}}}}`:                                          true,
		`{"request":{"selectors":{"alerts":{"transition":"health-child:t+1"}}}}`:                            true,
		`{"request":{"selectors":{"alerts":{"transition":null}}}}`:                                          true,
		`{"request":{"selectors":{"alerts":{}}}}`:                                                           true,
	} {
		v, err := ParseJSON([]byte(body))
		if err != nil {
			t.Fatal(err)
		}
		if err := alertsV2EchoAliased("")(v); (err != nil) != refused {
			t.Errorf("the echo's fact on %s: %v, want refused %t", body, err, refused)
		}
	}
	for body, refused := range map[string]bool{
		`{"transitions":[{"hostname":"health-child","transition_id":"health-child:t+1"},{"hostname":"parity-parent","transition_id":"t+2"}]}`: false,
		`{"transitions":[{"hostname":"health-child","transition_id":"t+1"}]}`:                                                                 true,
		`{"transitions":[{"hostname":"parity-parent","transition_id":"health-child:t+1"}]}`:                                                   true,
		`{"transitions":[{"hostname":"parity-parent","transition_id":"t?"}]}`:                                                                 true,
		`{"transitions":[{"transition_id":"t+1"}]}`:                                                                                           true,
		`{"items":{}}`: true,
	} {
		v, err := ParseJSON([]byte(body))
		if err != nil {
			t.Fatal(err)
		}
		if err := alertsV2HostIDs(v); (err != nil) != refused {
			t.Errorf("the hosts' ids fact on %s: %v, want refused %t", body, err, refused)
		}
	}
}

// dashNormClosersLPlant is one wrong answer planted in a recorded body (dashNormAlertsSub: a pattern with three
// groups, the second of which `to` rewrites); without `to`, the body's two first transitions the other way round.
type dashNormClosersLPlant struct {
	row, re string
	to      func(string) string
}

// apply is body with the plant.
func (c dashNormClosersLPlant) apply(t *testing.T, body string) string {
	t.Helper()
	return dashNormAlertsSub(t, body, c.re, func(g []string) string { return g[1] + c.to(g[2]) + g[3] })
}

// dashNormClosersLText is a plant's rewrite to a fixed text, and dashNormClosersLMoved one of a number by delta.
func dashNormClosersLText(s string) func(string) string { return func(string) string { return s } }
func dashNormClosersLMoved(delta int64) func(string) string {
	return func(n string) string {
		v, err := strconv.ParseInt(n, 10, 64)
		if err != nil {
			panic(fmt.Sprintf("%q is no number", n))
		}
		return strconv.FormatInt(v+delta, 10)
	}
}

// dashNormClosersLPlants are the named wrong answers, each with the paths the comparison reports when a candidate
// gives it (dashNormClosersLPlanted; a want ending ` …` names prefixes, every reported path under one of them and
// each of them reported) and whether the row's guard refuses it as the oracle's answer (dashNormClosersLRefused).
var dashNormClosersLPlants = map[string]struct {
	dashNormClosersLPlant
	want    string
	refused bool
}{
	// two hosts: the order of one pass's changes, the host of a row, the GUID as an option's name, an id another log has
	"two hosts: the newest pair the other way round": {dashNormClosersLPlant{"two-hosts/transitions", "", nil},
		"$.transitions[0]. $.transitions[1]. layout …", true},
	"two hosts: the child's newest row on localhost": {dashNormClosersLPlant{"two-hosts/transitions",
		`(?s)^(.*?"transitions":\[\{[^{}]*?"hostname":")(health-child)(")`, dashNormClosersLText("parity-parent")},
		"$.transitions[0].hostname", true},
	"two hosts: the child named by its GUID": {dashNormClosersLPlant{"two-hosts/transitions",
		`("id":"5a1e0000-0000-4000-8000-0000000000c1","name":")(health-child)(")`, dashNormClosersLText(healthChild.MachineGUID)},
		"$.facets[5].options[0].name", true},
	"two hosts: the child's newest id no log holds": {dashNormClosersLPlant{"two-hosts/transitions",
		`(?s)^(.*?"transitions":\[\{"gi":\d+,"alert":"hch_calc","transition_id":")([0-9a-f-]{36})(")`,
		dashNormClosersLText(dashNormTransitionsZero)}, "$.transitions[0]. …", true},
	"two hosts: the silenced change notified": {dashNormClosersLPlant{"two-hosts/transitions",
		`(?s)^(.*?"transitions":\[\{.*?"flags":\["PROCESSED",)("SILENCED")(,"SAVED"\])`, dashNormClosersLText(`"EXEC_RUN"`)},
		"$.transitions[0].notification.flags[1]", true},
	"two hosts: localhost's rows in the child's host table": {dashNormClosersLPlant{"two-hosts/transitions-nodes",
		`("items":\{"evaluated":)(3)(,)`, dashNormClosersLText("6")}, "$.items.evaluated", true},
	"two hosts: the child's index under its selector": {dashNormClosersLPlant{"two-hosts/nodes-child",
		`("nm":"health-child","ni":)(0)(\})`, dashNormClosersLText("1")}, "$.nodes[0].ni", true},
	"two hosts, mcp: the hostname left out": {dashNormClosersLPlant{"two-hosts/transitions-mcp",
		`(?s)^(.*?"transitions":\[\{[^{}]*?)("hostname":"health-child",)("instance")`, dashNormClosersLText("")},
		"$.transitions[0].<members> layout", true},
	// the echo of a transition
	"echo: the id in upper case": {dashNormClosersLPlant{"transitions/echo-transitions", `("transition":")([0-9a-f-]{36})(")`,
		strings.ToUpper}, "$.request.selectors.alerts.transition", true},
	"echo: the id without its dashes": {dashNormClosersLPlant{"transitions/echo-alerts", `("transition":")([0-9a-f-]{36})(")`,
		func(id string) string { return strings.ReplaceAll(id, "-", "") }}, "$.request.selectors.alerts.transition", true},
	"echo: an id no log holds": {dashNormClosersLPlant{"two-hosts/transition-child-debug", `("transition":")([0-9a-f-]{36})(")`,
		dashNormClosersLText(dashNormTransitionsZero)}, "$.request.selectors.alerts.transition", true},
	// the alerts' counts and the host prefilter
	"critical: counted as a warning": {dashNormClosersLPlant{"transitions/alerts-critical",
		`("nm":"hs_avg","sum":"","cr":)(1,"wr":0)(,)`, dashNormClosersLText("0,\"wr\":1")}, "$.alerts[2].cr $.alerts[2].wr", true},
	"half off: the host whose health never ran left out": {dashNormClosersLPlant{"half-off/raised",
		`("nm":"parity-parent","ni":0\})(,\{"mg":"[^"]*","nm":"health-child","ni":1\})(\])`, dashNormClosersLText("")},
		"$.nodes.length layout", true},
	"half off: that host listed under a window": {dashNormClosersLPlant{"half-off/raised-window",
		`("nm":"parity-parent","ni":0\})()(\])`, dashNormClosersLText(`,{"mg":"` + healthChild.MachineGUID + `","nm":"health-child","ni":1}`)},
		"$.nodes.length layout", true},
	"states: an UNDEFINED alert with a number counted as an error": {dashNormClosersLPlant{"states/alerts",
		`("nm":"hst_undef","sum":"","cr":0,"wr":0,"cl":0,"er":)(0)(,)`, dashNormClosersLText("1")}, "$.alerts[7].er", true},
	"states: the REMOVED alert counted as clear": {dashNormClosersLPlant{"states/alerts",
		`("nm":"hst_gone","sum":"","cr":0,"wr":0,"cl":)(0)(,)`, dashNormClosersLText("1")}, "$.alerts[1].cl", true},
	// the transitions' notification, chart and texts
	"delay: none": {dashNormClosersLPlant{"states/delay", `("delay":)(10)(,)`, dashNormClosersLText("0")},
		"$.transitions[0].notification.delay", true},
	"delay: its end at the change": {dashNormClosersLPlant{"states/delay", `("delay_up_to_time":)(\d+)(,)`, dashNormClosersLMoved(-10)},
		"$.transitions[0].notification.delay_up_to_time", true},
	"failed: no EXEC_FAILED": {dashNormClosersLPlant{"states/failed", `("EXEC_RUN",)("EXEC_FAILED",)("SAVED")`, dashNormClosersLText("")},
		"$.transitions[0].notification.flags[3] $.transitions[0].notification.flags.length layout", true},
	"failed: the code 0": {dashNormClosersLPlant{"states/failed", `("exec_code":)(127)(,)`, dashNormClosersLText("0")},
		"$.transitions[0].notification.exec_code", true},
	"named: its id as its name": {dashNormClosersLPlant{"states/named", `("instance_n":")(hst\.named)(")`, dashNormClosersLText("hst.idn")},
		"$.transitions[0].instance_n", true},
	"named, mcp: its id as its name": {dashNormClosersLPlant{"states/named-mcp", `("instance":")(hst\.named)(")`, dashNormClosersLText("hst.idn")},
		"$.transitions[0].instance", true},
	"named: the facet by the id": {dashNormClosersLPlant{"states/instance-name", `("items":\{"evaluated":5,"matched":)(1)(,)`,
		dashNormClosersLText("0")}, "$.items.matched", true},
	"idle: empty units for null": {dashNormClosersLPlant{"states/idle", `("units":)(null)(,)`, dashNormClosersLText(`""`)},
		"$.transitions[0].units", true},
	"long: the summary whole": {dashNormClosersLPlant{"states/long", `("summary":"s{510})(.)(")`, dashNormClosersLText("ézz")},
		"$.transitions[0].summary", true},
	"long: the context cut at 96 bytes": {dashNormClosersLPlant{"states/long", `("context":"hst\.c{91})()(")`, dashNormClosersLText("c")},
		"$.transitions[0].context", true},
	"long: the name whole": {dashNormClosersLPlant{"states/long", `("alert":"hst_long_n{38})()(")`, dashNormClosersLText("nnnnn")},
		"$.transitions[0].alert", true},
	"long: the recipient whole": {dashNormClosersLPlant{"states/long", `("to":"r{95})()(")`, dashNormClosersLText("rrrrr")},
		"$.transitions[0].notification.to", true},
	"long: the exec cut at 512 bytes": {dashNormClosersLPlant{"states/long", `("exec":"/dev/null/[^"]{501})()(")`, dashNormClosersLText("z")},
		"$.transitions[0].notification.exec", true},
}

// dashNormClosersLPlanted plants each wrong answer in the candidate's recorded body and wants it reported at its paths
// (dashNormClosersLPlants).
func dashNormClosersLPlanted(t *testing.T) {
	t.Helper()
	for _, name := range slices.Sorted(maps.Keys(dashNormClosersLPlants)) {
		c := dashNormClosersLPlants[name]
		r := dashNormClosersLRows[c.row]
		body := r.bodies[1]
		if c.to == nil {
			body = dashNormClosersLSwap(t, body, "transitions", 0, 1)
		} else {
			body = c.apply(t, body)
		}
		got := dashNormClosersLPairOf(t, r, r.bodies[0], body).diffs()
		if prefixes, ok := strings.CutSuffix(c.want, " …"); ok {
			paths := strings.Fields(got)
			for _, prefix := range strings.Fields(prefixes) {
				if !slices.ContainsFunc(paths, func(p string) bool { return strings.HasPrefix(p, prefix) }) {
					t.Errorf("%s: differences at %q, none under %s", name, got, prefix)
				}
			}
			for _, p := range paths {
				if !slices.ContainsFunc(strings.Fields(prefixes), func(prefix string) bool { return strings.HasPrefix(p, prefix) }) {
					t.Errorf("%s: a difference at %s, outside %s", name, p, prefixes)
				}
			}
			continue
		}
		if got != c.want {
			t.Errorf("%s: differences at %q, want %q", name, got, c.want)
		}
	}
}

// dashNormClosersLRefused gives each wrong answer that its row's guard must refuse as the oracle's answer, normalised
// as the check hands it over (dashNormClosersLPlants); the others the guard cannot see (a member the guard does not
// read, a second the render names alike) are the comparison's alone.
func dashNormClosersLRefused(t *testing.T, guards map[string]func(Value) error) {
	t.Helper()
	for _, name := range slices.Sorted(maps.Keys(dashNormClosersLPlants)) {
		c := dashNormClosersLPlants[name]
		r := dashNormClosersLRows[c.row]
		body := r.bodies[0]
		if c.to == nil {
			body = dashNormClosersLSwap(t, body, "transitions", 0, 1)
		} else {
			body = c.apply(t, body)
		}
		err := guards[c.row](dashNormClosersLPairOf(t, r, body, r.bodies[1]).doc[0])
		if c.refused && err == nil {
			t.Errorf("%s: the guard of %s takes it as the oracle's answer", name, c.row)
		}
		if !c.refused && err != nil {
			t.Errorf("%s: the guard of %s refuses it (%v): the plant is the comparison's, say so", name, c.row, err)
		}
	}
}
