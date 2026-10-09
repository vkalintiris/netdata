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

// dashNormTransitionsCases are the transitions rows of D234 F7 by case, as `api.v2-alert-transitions` asks them
// (`TestAlertsV2/transitions` and `/rules`), with the switches' seconds and the clock of the recorded run. A row that
// names a transition has no id here: its guard does not read one.
func dashNormTransitionsCases(sw [4]int64, now int64) map[string][]v2Req {
	var ids [2]string
	return map[string][]v2Req{
		"transitions": slices.Concat(alertsV2TransitionsRows(sw), alertsV2TransitionsOneRows(ids, ids),
			[]v2Req{alertsV2TransitionsAgoRow(now, sw)}, alertsV2TimeoutRows),
		"rules": slices.Concat(alertsV2RulesRows(), alertsV2RulesOneRows(ids)),
	}
}

// dashNormTransitionsClock reads the recorded run's seconds out of two recorded targets: the second and the third
// switch, three seconds after the two ends of `window-abs`, and the second `window-ago` was made in, whose `before`
// counts back to three seconds before the third switch.
func dashNormTransitionsClock(t *testing.T) (sw [4]int64, now int64) {
	t.Helper()
	ends := regexp.MustCompile(`after=(\d+)&before=(\d+)$`).FindStringSubmatch(dashNormTransitionsRows["transitions/window-abs"].target)
	ago := regexp.MustCompile(`before=-(\d+)$`).FindStringSubmatch(dashNormTransitionsRows["transitions/window-ago"].target)
	if ends == nil || ago == nil {
		t.Fatalf("the recorded targets of window-abs and window-ago do not hold their seconds")
	}
	g2, _ := strconv.ParseInt(ends[1], 10, 64)
	g3, _ := strconv.ParseInt(ends[2], 10, 64)
	n, _ := strconv.ParseInt(ago[1], 10, 64)
	return [4]int64{0, 0, g2 + 3, g3 + 3}, g3 + n
}

// dashNormTransitionsRowOf is the recorded row of a key with its answer: its own, or the one of the row it names
// (dashNormTransitionsRow.same), under its own target.
func dashNormTransitionsRowOf(key string) dashNormTransitionsRow {
	r := dashNormTransitionsRows[key]
	if r.same != "" {
		target := r.target
		r = dashNormTransitionsRows[r.same]
		r.target = target
	}
	return r
}

// dashNormTransitionsPairOf normalises the bodies o and c as the recorded row r's family does: the alerts family
// with each side's recorded log, each body in the seconds its own request was in flight (dashNormAlertsPair).
func dashNormTransitionsPairOf(t *testing.T, r dashNormTransitionsRow, o, c string) dashNormAlertsPair {
	t.Helper()
	p := dashNormAlertsPair{fam: alertsV2Family([2]*healthNorm{dashNormAlertsNorm(), dashNormAlertsNorm()},
		func(i int) ([]byte, error) { return []byte(dashNormTransitionsLogs[r.logs[i]]), nil })}
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

// dashNormTransitionsAgainst is the pair of the recorded row r's oracle answer and a candidate's answer made of the
// recorded candidate's by `plant`; for a row whose candidate side was not recorded, of the oracle's own answer,
// read against the oracle's log.
func dashNormTransitionsAgainst(t *testing.T, r dashNormTransitionsRow, plant func(body string) string) dashNormAlertsPair {
	t.Helper()
	if r.bodies[1] == "" {
		r.bodies[1], r.flight[1], r.logs[1] = r.bodies[0], r.flight[0], r.logs[0]
	}
	return dashNormTransitionsPairOf(t, r, r.bodies[0], plant(r.bodies[1]))
}

// testDashNormTransitions pins the transitions rows of D234 F7 (H37) on the answers and the alert logs of one
// C-against-C run (dash_norm_transitions_data_test.go): each row's guard and family on its own recorded pair, what
// each guard refuses, what the render names of a transition that is written without its id, with dates or by its
// long key, each wrong answer planted, the guards' own judges, and the timeout's rows on two stub agents.
func testDashNormTransitions(t *testing.T) {
	sw, now := dashNormTransitionsClock(t)
	guards, targets := map[string]func(Value) error{}, map[string]string{}
	var plain, raw []string
	for name, rows := range dashNormTransitionsCases(sw, now) {
		for _, req := range rows {
			key := name + "/" + req.name
			if req.guard == nil || (req.status != "200" && req.status != "504") {
				t.Errorf("%s: a row without a guard, or one that wants another status than 200 or 504", key)
			}
			if _, twice := guards[key]; twice {
				t.Errorf("%s: two rows of one name", key)
			}
			guards[key], targets[key] = req.guard, req.target
			if req.status == "200" {
				plain = append(plain, key)
			} else {
				raw = append(raw, key)
			}
		}
	}
	slices.Sort(plain)
	slices.Sort(raw)
	// every row was recorded, and nothing else
	if got := slices.Sorted(maps.Keys(dashNormTransitionsRows)); !slices.Equal(got, plain) {
		t.Fatalf("the recorded rows are %q, the checks' rows %q", got, plain)
	}
	if got := slices.Sorted(maps.Keys(dashNormTransitionsRaw)); !slices.Equal(got, raw) {
		t.Fatalf("the recorded rows that answer no JSON are %q, the checks' %q", got, raw)
	}

	// Each recorded pair shows no difference under its family, the seconds its render named are within the bound,
	// and the row's guard takes the oracle's answer as the check hands it over: normalised. A row whose candidate
	// side was not recorded is pinned by its guard and its target; a row whose answer was another row's but for its
	// timings is pinned on that row's.
	pairs := map[string]dashNormAlertsPair{}
	var both []string
	for _, key := range plain {
		if same := dashNormTransitionsRows[key].same; same != "" {
			if named, own := dashNormTransitionsRows[same], dashNormTransitionsRows[key]; named.same != "" || named.bodies[0] == "" ||
				own.bodies[0] != "" || strings.Split(same, "/")[0] != strings.Split(key, "/")[0] {
				t.Fatalf("%s names the answer of %s, which is no recorded answer of its case, or has one of its own", key, same)
			}
		}
		r := dashNormTransitionsRowOf(key)
		c := r.bodies[1]
		if c == "" {
			c = r.bodies[0]
			r.logs[1], r.flight[1] = r.logs[0], r.flight[0]
		} else if dashNormTransitionsRows[key].same == "" {
			both = append(both, key)
		}
		p := dashNormTransitionsPairOf(t, r, r.bodies[0], c)
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
	// the rows recorded on both sides: each form of the answer the render reads, and each case
	if !slices.Equal(both, dashNormTransitionsBoth) {
		t.Errorf("the rows recorded on both sides are %q, want %q", both, dashNormTransitionsBoth)
	}

	// What each guard refuses: the answer of every other row of its case, but the rows named here, whose answers hold
	// what the guard reads.
	accepted := map[string][]string{}
	for _, key := range plain {
		name, _, _ := strings.Cut(key, "/")
		for _, other := range plain {
			if other != key && strings.HasPrefix(other, name+"/") && guards[key](pairs[other].doc[0]) == nil {
				accepted[key] = append(accepted[key], strings.TrimPrefix(other, name+"/"))
			}
		}
	}
	if want := dashNormTransitionsAccepted(); !maps.EqualFunc(accepted, want, slices.Equal[[]string]) {
		for _, key := range plain {
			if !slices.Equal(accepted[key], want[key]) {
				t.Errorf("%s: its guard also takes the answers of %q, want %q", key, accepted[key], want[key])
			}
		}
	}

	dashNormTransitionsRender(t, pairs, sw)
	dashNormTransitionsPlanted(t)
	dashNormTransitionsHelpers(t, pairs)
	dashNormTransitionsTimeout(t, guards)
}

// dashNormTransitionsBoth are the rows recorded on both sides: the pair of each form of the answer the render reads
// (plain, dated, by the long key, the MCP form with rules and with the echo), of the window counted from now, and
// of the `rules` case (the half character, an entry no window lists).
var dashNormTransitionsBoth = []string{"rules/config-mcp", "rules/idle", "transitions/config-mcp", "transitions/debug-mcp",
	"transitions/facets", "transitions/first-rfc3339", "transitions/long", "transitions/mcp", "transitions/mcp-rfc3339",
	"transitions/one-mcp", "transitions/rfc3339", "transitions/window-ago"}

// dashNormTransitionsAccepted are, per row, the other rows of its case whose recorded answers its guard takes too.
// No guard of the `rules` case takes another row's answer.
func dashNormTransitionsAccepted() map[string][]string {
	out := map[string][]string{
		// hs_calc's rows alone, by the statement's filter: the newest of them, with its rule too
		"transitions/alert-context": {"config-debug"},
		// the newest row of the nine with every count, in another form of the answer: the guard that names the plain
		// form's members takes the rows of that form, and the one that reads the dates' marks the other dated answer
		"transitions/pretty":  {"context-later", "facet-node", "facet-star", "facet-values", "facet-wordless", "long", "no-last", "rfc3339", "unread"},
		"transitions/rfc3339": {"config-debug-rfc3339"},
		// one request in two spellings: an anchor that wraps to 0 is no anchor
		"transitions/anchor-wrap": {"debug-mcp"},
		"transitions/debug-mcp":   {"anchor-wrap"},
		// every row rejected by its alert's name alone
		"transitions/facet-blank":    {"facet-negative"},
		"transitions/facet-negative": {"facet-blank"},
		// no row and no host
		"transitions/one-text":        {"timeout-no-host"},
		"transitions/timeout-no-host": {"one-text"},
	}
	// The rows that ask one answer by different requests take one another's, and with them what `more` names: the
	// same rows in another form of the answer, whose guards read more than these do.
	for _, group := range []struct{ rows, more []string }{
		// nothing: no row of the window, no host, or no window
		{rows: []string{"alert-case", "alert-pattern", "context-pattern", "contexts-later", "contexts-other", "no-window", "nodes-none",
			"scope-contexts-none"}},
		// the newest row of the nine, every count
		{rows: []string{"context-later", "facet-node", "facet-star", "facet-values", "facet-wordless", "no-last", "unread"},
			more: []string{"config-debug-rfc3339", "debug-options", "long", "pretty", "rfc3339"}},
		// the newest two
		{rows: []string{"anchor-zero", "context", "dashboard"}, more: []string{"config-last2"}},
		// hs_calc's change to WARNING by its id
		{rows: []string{"one-appended", "one-bare", "one-filters"}, more: []string{"one-config"}},
	} {
		for _, row := range group.rows {
			also := slices.Clone(group.more)
			for _, other := range group.rows {
				if other != row {
					also = append(also, other)
				}
			}
			slices.Sort(also)
			out["transitions/"+row] = also
		}
	}
	return out
}

// dashNormTransitionsRender pins what the render wrote into the recorded oracle answers of the forms the rows add:
// a transition without its id (`options=mcp`) named by its change, a transition's seconds as dates, the global id
// by its long key; and what it leaves.
func dashNormTransitionsRender(t *testing.T, pairs map[string]dashNormAlertsPair, sw [4]int64) {
	t.Helper()
	// hs_avg's newest transition in the MCP form, up to its notification's flags; %[1]s: what ends a date's mark
	const mcpAvg = `{"gi":"G","alert":"hs_avg","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent",` +
		`"instance":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":"WHEN%[1]s",` +
		`"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},` +
		`"old":{"status":"CRITICAL","value":95,"duration":"DURATION","raised_duration":"NON_CLEAR"},` +
		`"notification":{"when":"EXEC_RUN%[1]s","delay":0,"delay_up_to_time":"DELAY_UP_TO%[1]s",`
	for key, wants := range map[string][]string{
		"transitions/config-mcp": {fmt.Sprintf(mcpAvg, ""), `{"gi":"G","alert":"hs_max","config_hash_id":`,
			`{"gi":"G","alert":"hs_calc","config_hash_id":`},
		"transitions/mcp-rfc3339": {fmt.Sprintf(mcpAvg, alertsV2Dated)},
		"transitions/rfc3339": {`"type":null,"when":"WHEN` + alertsV2Dated + `","info":`, `"notification":{"when":"EXEC_RUN` +
			alertsV2Dated + `","delay":0,"delay_up_to_time":"DELAY_UP_TO` + alertsV2Dated + `",`},
		"transitions/long": {`"transitions":[{"global_id":"G","alert":"hs_avg","transition_id":"t+`},
		"transitions/first-rfc3339": {`"old":{"status":"UNINITIALIZED","value":0,"duration":0,"raised_duration":0},` +
			`"notification":{"when":null,"delay":0,"delay_up_to_time":"DELAY_UP_TO` + alertsV2Dated + `",`},
		"transitions/one-mcp": {`{"transitions":[{"gi":"G","alert":"hs_calc","config_hash_id":`, `"type":null,"when":"WHEN","info":`,
			`"old":{"status":"CLEAR","value":10,"duration":"DURATION","raised_duration":0},"notification":{"when":"EXEC_RUN",`},
		// the echo's anchor is the request's number, which the render leaves
		"transitions/debug": {fmt.Sprintf(`"anchor_gi":%d,`, sw[2]*1_000_000), `"gi":"G",`},
		// a return to CLEAR that is not notified: no notification second, and the 0 stays; the half character written out
		"rules/config-mcp": {`{"gi":"G","alert":"hr_plain","config_hash_id":`, `"notification":{"when":0,"delay":0,` +
			`"delay_up_to_time":"DELAY_UP_TO","flags":["PROCESSED","SAVED","NO_CLEAR_NOTIFICATION"],`,
			strings.Repeat("i", 510) + string(alertsV2Marker) + `c3","summary":"",`},
	} {
		for _, want := range wants {
			if got := string(pairs[key].body[0]); !strings.Contains(got, want) {
				t.Errorf("%s, rendered, does not hold %s: %s", key, want, got)
			}
		}
	}
	// what the render leaves: the answers that hold no transition, byte for byte
	for _, key := range []string{"transitions/one-text", "transitions/last-negative", "transitions/facet-reject", "rules/context-other"} {
		if got := string(pairs[key].body[0]); got != dashNormTransitionsRowOf(key).bodies[0] {
			t.Errorf("%s: the render changed an answer that holds no transition: %s", key, got)
		}
	}
}

// dashNormTransitionsPlanted plants each wrong answer in the candidate's recorded body (the oracle's own, for a row
// recorded on one side: dashNormTransitionsAgainst), and wants it reported at its paths and nowhere else; an empty
// want is an answer that must show no difference. A plant is a pattern with three groups, the second of which `to`
// rewrites (dashNormAlertsSub): so the plants hold over another recorded run.
func dashNormTransitionsPlanted(t *testing.T) {
	t.Helper()
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
	number := func(d string) string {
		v, date, ok := alertsV2Second(d)
		if !ok || !date {
			t.Fatalf("%q is no date", d)
		}
		return strconv.FormatInt(v, 10)
	}
	text := func(s string) func(string) string { return func(string) string { return s } }
	const (
		// the MCP form of hs_calc's transition, the third of its answer, up to a member
		calc = `"alert":"hs_calc","config_hash_id":[^\]]*?`
		tr0  = "$.transitions[0]."
		tr2  = "$.transitions[2]."
		// everything the render names of that transition: its entry's seconds and spans (none of them 0)
		named = tr2 + "gi " + tr2 + "when " + tr2 + "old.duration " + tr2 + "old.raised_duration " + tr2 + "notification.when " +
			tr2 + "notification.delay_up_to_time"
	)
	for name, c := range map[string]struct {
		row, re string
		to      func(string) string
		want    string
	}{
		// a transition without its id: its entry is the one of its alert's change at its second, or none
		"mcp: the second a second late": {"transitions/config-mcp", `(` + calc + `"when":)(\d+)(,)`, moved(1), named},
		// (hs_avg and hs_max return to CLEAR in one second: under the other's name the transition finds the other's
		// entry, whose spans are not its own)
		"mcp: the name of the alert that changed alike in its second": {"transitions/config-mcp",
			`("gi":\d+,"alert":")(hs_avg)(")`, text("hs_max"), tr0 + "alert " + tr0 + "old.duration " + tr0 + "old.raised_duration"},
		"mcp: another alert's name": {"transitions/config-mcp", `("gi":\d+,"alert":")(hs_calc)(")`, text("hs_max"),
			tr2 + "gi " + tr2 + "alert " + named[len(tr2+"gi "):]},
		"mcp: another new status": {"transitions/config-mcp", `(` + calc + `"new":\{"status":")(CLEAR)(")`, text("WARNING"),
			tr2 + "gi " + tr2 + "when " + tr2 + "new.status " + named[len(tr2+"gi "+tr2+"when "):]},
		"mcp: another old status": {"transitions/config-mcp", `(` + calc + `"old":\{"status":")(CRITICAL)(")`, text("WARNING"),
			tr2 + "gi " + tr2 + "when " + tr2 + "old.status " + named[len(tr2+"gi "+tr2+"when "):]},
		"mcp: the duration off by one": {"transitions/config-mcp", `(` + calc + `"old":\{"status":"CRITICAL","value":95,"duration":)(\d+)(,)`,
			moved(1), tr2 + "old.duration"},
		"mcp: the raised duration off by one": {"transitions/config-mcp", `(` + calc + `"raised_duration":)(\d+)(\})`, moved(-1),
			tr2 + "old.raised_duration"},
		"mcp: the global id three seconds late": {"transitions/config-mcp", `("gi":)(\d+)(,"alert":"hs_calc")`, moved(3_000_000), tr2 + "gi"},
		"mcp: the global id two seconds late":   {"transitions/config-mcp", `("gi":)(\d+)(,"alert":"hs_calc")`, moved(2_000_000), ""},
		"mcp: the notification a second late": {"transitions/config-mcp", `(` + calc + `"notification":\{"when":)(\d+)(,)`, moved(1),
			tr2 + "notification.when"},
		"mcp: the delay's end a second early": {"transitions/config-mcp", `(` + calc + `"delay_up_to_time":)(\d+)(,)`, moved(-1),
			tr2 + "notification.delay_up_to_time"},
		"mcp: the id the plain form has": {"transitions/config-mcp", `("alert":"hs_calc",)()("config_hash_id")`,
			text(`"transition_id":"11111111-2222-4333-8444-555555555555",`), tr2 + "<members> " + named + " layout"},
		"mcp, dated: the second a second late": {"transitions/mcp-rfc3339", `("type":null,"when":")([^"]*)(")`, dated(1),
			tr0 + "gi " + tr0 + "when " + tr0 + "old.duration " + tr0 + "old.raised_duration " + tr0 + "notification.when " + tr0 +
				"notification.delay_up_to_time"},
		"mcp, dated: the second as a number": {"transitions/mcp-rfc3339", `("type":null,"when":)("[^"]*")()`, number, tr0 + "when"},
		// the dates of the plain form
		"dates: the second as a number":         {"transitions/rfc3339", `("type":null,"when":)("[^"]*")()`, number, tr0 + "when"},
		"dates: the second a second late":       {"transitions/rfc3339", `("type":null,"when":")([^"]*)(")`, dated(1), tr0 + "when"},
		"dates: the notification a second late": {"transitions/rfc3339", `("notification":\{"when":")([^"]*)(")`, dated(1), tr0 + "notification.when"},
		"dates: a fraction in the delay's end": {"transitions/rfc3339", `("delay_up_to_time":"[^"]*)()(Z")`, text(".000"),
			tr0 + "notification.delay_up_to_time"},
		"dates: a null for a notification that ran": {"transitions/rfc3339", `("notification":\{"when":)("[^"]*")(,)`, text("null"),
			tr0 + "notification.when"},
		"dates: a date for a notification that never ran": {"transitions/first-rfc3339", `("notification":\{"when":)(null)(,)`,
			text(`"1970-01-01T00:00:00Z"`), tr0 + "notification.when"},
		// the long key
		"long keys: a global id in seconds": {"transitions/long", `("global_id":\d+)(\d{6})(,)`, text(""), tr0 + "global_id"},
		"long keys: the short key":          {"transitions/long", `(\{")(global_id)(":\d+,)`, text("gi"), tr0 + "<members>"},
		// the facets
		"facets: a count off by one": {"transitions/facets", `("id":"f_status".*?"id":"CLEAR","name":"CLEAR","count":)(\d+)(\})`, moved(1),
			"$.facets[0].options[0].count"},
		"facets: a host named by its GUID": {"transitions/facets", `("id":"5a1e[^"]*","name":")(parity-parent)(")`,
			text(parentIdentity.MachineGUID), "$.facets[5].options[0].name"},
		"facets: two options the other way round": {"transitions/facets", `("options":\[)(\{"id":"hs_avg","name":"hs_avg","count":1\},` +
			`\{"id":"hs_max","name":"hs_max","count":1\})(,)`, func(two string) string {
			a, b, _ := strings.Cut(two, "},")
			return b + "," + a + "}"
		}, "$.facets[6].options[0].id $.facets[6].options[0].name $.facets[6].options[1].id $.facets[6].options[1].name"},
		"facets: a matched row less": {"transitions/facets", `("matched":)(\d+)(,)`, moved(-1), "$.items.matched"},
		// the rules
		"rules: the stored order": {"transitions/config", `("configurations":\[)(.*)(\],"items")`, func(rules string) string {
			three := regexp.MustCompile(`^(\{"name":"hs_avg".*\}),(\{"name":"hs_max".*\}),(\{"name":"hs_calc".*\})$`).FindStringSubmatch(rules)
			if three == nil {
				t.Fatalf("the recorded rules are not the three, in the order the transitions show them: %s", rules)
			}
			return three[3] + "," + three[2] + "," + three[1]
		}, "rules"},
		"rules: green as a number under debug": {"transitions/config-debug", `("green":)(null)(,)`, text("20"),
			"$.configurations[0].status.green"},
		"rules: a zero lookup end as a number under rfc3339": {"transitions/config-debug-rfc3339", `("after":-5,\s*"before":)(null)(,)`,
			text("0"), "$.configurations[0].value.db.before"},
		"rules: a relative lookup start as a date": {"transitions/config-debug-rfc3339", `("db":\{\s*"after":)(-5)(,)`,
			text(`"1969-12-31T23:59:55Z"`), "$.configurations[0].value.db.after"},
		"rules: a name under mcp": {"transitions/config-mcp", `("configurations":\[\{)()("config_hash_id")`, text(`"name":"hs_avg",`),
			"$.configurations[0].<members> layout"},
		// the echo and the keep's counts
		"echo: another last":     {"transitions/debug-mcp", `("last":)(1)(,)`, text("2"), "$.request.selectors.alerts.last"},
		"echo: instances listed": {"transitions/debug-options", `("options":\[)()("minify")`, text(`"instances",`), "echo"},
		"echo: instances in their place": {"transitions/debug-options", `("options":\["minify","debug",)()("values")`, text(`"instances",`),
			"echo"},
		"echo: the scope of contexts": {"transitions/debug-all", `("scope_nodes":"\*")()(\s*\})`, text(`,"scope_contexts":"hsig*"`),
			"$.request.scope.<members> layout"},
		"stats: a skip less": {"transitions/debug", `("skips_after":)(2)(\s)`, text("1"), "$.stats.skips_after"},
		"echo: the anchor not wrapped": {"transitions/anchor-wrap", `("anchor_gi":)(0)(,)`, text("18446744073709551615"),
			"$.request.selectors.alerts.anchor_gi"},
		// the texts of a transition
		"texts: the class whole": {"rules/window", `("alert":"hr_two"[^{]*?"classification":"Latency_c{39})()(".*?"new":\{"status":"CLEAR")`,
			text("cccccccc"), "$.transitions[1].classification"},
		"texts: the info cut at a character": {"rules/window", `("info":"i{510})(.)(","summary":"","units":"units","new":\{"status":"CLEAR")`,
			text(""), tr0 + "info"},
		"texts: U+FFFD for the half character": {"rules/window", `("info":"i{510})(.)(","summary":"","units":"units","new":\{"status":"CLEAR")`,
			text("�"), tr0 + "info"},
		"texts: no units for a rule without": {"rules/window", `("summary":"","units":)("units")(,"new":\{"status":"CLEAR")`, text("null"),
			tr0 + "units"},
		"texts: a summary as written": {"rules/window", `("summary":")(tpl fam r)(","units":"things","new":\{"status":"CLEAR")`,
			text("tpl ${family}"), "$.transitions[2].summary"},
		"rules: a template's name": {"rules/config", `("configurations":.*?\{"name":)(null)(,)`, text(`"hr_tpl"`),
			"$.configurations[2].name"},
		"rules: a template on its context": {"rules/config", `("selectors":\{"type":"template","on":")(hr_tpl)(")`, text("hrul.ctx"),
			"$.configurations[2].selectors.on"},
		"rules: a threshold as written": {"rules/config", `("warn":"\$this > 50","crit":")(\$this > 80)(")`, text("$this > $red"),
			"$.configurations[1].status.crit"},
		"rules: green where the rule has one": {"rules/config", `("status":\{)()("warn":"\$this > 50","crit":")`, text(`"green":20,"red":80,`),
			"$.configurations[1].status.<members> layout"},
	} {
		got := dashNormTransitionsAgainst(t, dashNormTransitionsRowOf(c.row), func(body string) string {
			return dashNormAlertsSub(t, body, c.re, func(g []string) string { return g[1] + c.to(g[2]) + g[3] })
		}).diffs()
		switch c.want {
		case "rules":
			// each of the two rules that changed places differs in several members: at the first and the third alone
			for _, path := range strings.Fields(strings.TrimSuffix(got, " layout")) {
				if !strings.HasPrefix(path, "$.configurations[0].") && !strings.HasPrefix(path, "$.configurations[2].") {
					t.Errorf("%s: a difference at %q", name, path)
				}
			}
			if !strings.Contains(got, "$.configurations[0].name") || !strings.Contains(got, "$.configurations[2].name") ||
				!strings.HasSuffix(got, " layout") {
				t.Errorf("%s: differences at %q, want the two rules' names among them, and the layout (a rule with a lookup "+
					"has other members than one with a calculation)", name, got)
			}
		case "echo":
			if !strings.HasPrefix(got, "$.request.options") {
				t.Errorf("%s: differences at %q, want the echo's options", name, got)
			}
		default:
			if got != c.want {
				t.Errorf("%s: differences at %q, want %q", name, got, c.want)
			}
		}
	}
}

// dashNormTransitionsZero is a transition id no log holds.
const dashNormTransitionsZero = "00000000-0000-4000-8000-000000000000"

// dashNormTransitionsHelpers pins the guards' own judges and the lookup of an entry by its change, on hand-made
// values and on the recorded answers.
func dashNormTransitionsHelpers(t *testing.T, pairs map[string]dashNormAlertsPair) {
	t.Helper()
	// the entry of a change: exactly one, by the alert's name, both statuses and the second
	log, err := alertsV2LogOf([]byte(`[{"transition_id":"a","name":"x","status":"WARNING","old_status":"CLEAR","when":10},` +
		`{"transition_id":"b","name":"x","status":"CRITICAL","old_status":"WARNING","when":10},` +
		`{"transition_id":"c","name":"y","status":"WARNING","old_status":"CLEAR","when":10},` +
		`{"transition_id":"d","name":"x","status":"WARNING","old_status":"CLEAR","when":11},` +
		`{"transition_id":"e","name":"z","status":"UNINITIALIZED","old_status":"REMOVED","when":5},` +
		`{"transition_id":"f","name":"z","status":"UNINITIALIZED","old_status":"REMOVED","when":5}]`))
	if err != nil {
		t.Fatal(err)
	}
	for want, c := range map[string]struct {
		name, old, status string
		when              int64
	}{
		"a": {"x", "CLEAR", "WARNING", 10}, "b": {"x", "WARNING", "CRITICAL", 10}, "c": {"y", "CLEAR", "WARNING", 10},
		"d": {"x", "CLEAR", "WARNING", 11},
		// no entry: another second, another name, the statuses the other way round, another alert's change
		"none 1": {"x", "CLEAR", "WARNING", 12}, "none 2": {"w", "CLEAR", "WARNING", 10}, "none 3": {"x", "WARNING", "CLEAR", 10},
		"none 4": {"y", "WARNING", "CRITICAL", 10}, "none 5": {"x", "CLEAR", "CRITICAL", 10}, "none 6": {"x", "WARNING", "WARNING", 10},
		// two entries of one change: neither
		"none 7": {"z", "REMOVED", "UNINITIALIZED", 5},
	} {
		got, found := log.changed(c.name, c.old, c.status, c.when)
		if strings.HasPrefix(want, "none") != !found || (found && got.Tid != want) {
			t.Errorf("the entry of %s from %s to %s at %d is %q (found %t), want %s", c.name, c.old, c.status, c.when, got.Tid, found, want)
		}
	}
	// What the render names by a change: a transition without an id, by either key of its global id, and with its
	// second as a date. One with an id the log does not hold stays as it is, though its change is an entry's; so
	// does one whose alert is not the member after its global id (C writes it there), and one whose change is no
	// entry's.
	n := dashNormAlertsNorm()
	const change = `"new":{"status":"WARNING","value":1},"old":{"status":"CLEAR","value":0,"duration":0}}]}`
	for body, want := range map[string]string{
		`{"transitions":[{"gi":10000001,"alert":"x","when":10,` + change:        `{"transitions":[{"gi":"G","alert":"x","when":"WHEN",`,
		`{"transitions":[{"global_id":10000001,"alert":"x","when":10,` + change: `{"transitions":[{"global_id":"G","alert":"x","when":"WHEN",`,
		`{"transitions":[{"gi":10000001,"alert":"x","when":"1970-01-01T00:00:10Z",` + change: `{"transitions":[{"gi":"G","alert":"x",` +
			`"when":"WHEN` + alertsV2Dated + `",`,
		`{"transitions":[{"gi":10000001,"alert":"x","transition_id":"` + dashNormTransitionsZero + `","when":10,` + change: `{"transitions":[{"gi":10000001,` +
			`"alert":"x","transition_id":"` + dashNormTransitionsZero + `","when":10,`,
		`{"transitions":[{"gi":10000001,"nm":"y","alert":"x","when":10,` + change: `{"transitions":[{"gi":10000001,"nm":"y","alert":"x","when":10,`,
		`{"transitions":[{"gi":10000001,"alert":"w","when":10,` + change:          `{"transitions":[{"gi":10000001,"alert":"w","when":10,`,
		`{"transitions":[{"gi":10000001,"alert":"x","when":12,` + change:          `{"transitions":[{"gi":10000001,"alert":"x","when":12,`,
		`{"transitions":[{"gi":10000001,"alert":"x","when":"10",` + change:        `{"transitions":[{"gi":10000001,"alert":"x","when":"10",`,
	} {
		if got, _ := alertsV2Render(n, log, [2]int64{10, 10}, []byte(body)); !strings.HasPrefix(string(got), want) {
			t.Errorf("rendered %s, want it to start %s", got, want)
		}
	}

	// the facets' guard: C's answer, and what it refuses
	facets := pairs["transitions/facets"].doc[0]
	want := map[string]string{"f_status": "CLEAR=2 CRITICAL=2 WARNING=2", "f_class": "unknown=2", "f_type": "unknown=2",
		"f_component": "unknown=2", "f_role": "root=2", "f_node": parentIdentity.MachineGUID + "(" + parentIdentity.Hostname + ")=2",
		"f_alert": "hs_avg=1 hs_max=1 hs_calc=1", "f_instance": "hsig.values=2", "f_context": "hsig.ctx=2"}
	if err := alertsV2Facets(want)(facets); err != nil {
		t.Errorf("the facets' guard refuses C's answer: %v", err)
	}
	if err := alertsV2Facets(nil)(facets); err == nil {
		t.Errorf("the guard of an answer without facets takes one with them")
	}
	if err := alertsV2Facets(nil)(pairs["transitions/mcp"].doc[0]); err != nil {
		t.Errorf("the guard of an answer without facets refuses the MCP form: %v", err)
	}
	if err := alertsV2Facets(want)(pairs["transitions/mcp"].doc[0]); err == nil {
		t.Errorf("the facets' guard takes an answer without facets")
	}
	// the facets of a request that evaluated no row have no option
	if err := alertsV2SigFacets("", "", 0, nil)(facets); err == nil {
		t.Errorf("the guard of facets without options takes facets with them")
	}
	if err := alertsV2SigFacets("", "", 0, nil)(pairs["transitions/no-window"].doc[0]); err != nil {
		t.Errorf("the guard of facets without options refuses C's: %v", err)
	}
	for id, other := range map[string]string{
		"f_status": "CRITICAL=2 CLEAR=2 WARNING=2", "f_alert": "hs_avg=1 hs_max=1", "f_node": parentIdentity.MachineGUID + "=2",
		"f_context": "", "f_role": "root=2 silent=0",
	} {
		wrong := maps.Clone(want)
		wrong[id] = other
		if err := alertsV2Facets(wrong)(facets); err == nil {
			t.Errorf("the facets' guard takes %s as %q", id, other)
		}
	}
	body := string(pairs["transitions/facets"].body[0])
	const status, class = `{"id":"f_status","name":"Alert Status","order":1,`, `{"id":"f_class","name":"Alert Class","order":4,`
	for name, c := range map[string][2]string{
		"another name":               {`"name":"Alert Status"`, `"name":"Status"`},
		"another order":              {`"name":"Alert Status","order":1,`, `"name":"Alert Status","order":2,`},
		"an order as a text":         {`"name":"Alert Status","order":1,`, `"name":"Alert Status","order":"1",`},
		"a count as a text":          {`"id":"CLEAR","name":"CLEAR","count":2}`, `"id":"CLEAR","name":"CLEAR","count":"2"}`},
		"an option without its name": {`{"id":"CLEAR","name":"CLEAR","count":2}`, `{"id":"CLEAR","count":2}`},
		"a member more":              {`"name":"Alert Status","order":1,`, `"name":"Alert Status","order":1,"more":0,`},
		"a facet less":               {`,{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":2}]}`, ``},
		"no options":                 {`"name":"Alert Status","order":1,"options":[`, `"name":"Alert Status","order":1,"opts":[`},
		// two facets that change places: their options stay each one's own
		"two facets the other way round": {status, class},
	} {
		wrong := strings.Replace(body, c[0], c[1], 1)
		if c[0] == status {
			wrong = strings.Replace(strings.Replace(strings.Replace(body, status, "<status>", 1), class, status, 1), "<status>", class, 1)
		}
		v, err := ParseJSON([]byte(wrong))
		if wrong == body || err != nil {
			t.Fatalf("the facets with %s: the plant did not apply, or made no JSON: %v", name, err)
		}
		if err := alertsV2Facets(want)(v); err == nil {
			t.Errorf("the facets' guard takes %s", name)
		}
	}

	// the rules' guard: the rules in the order the transitions first show them, and their names
	rules := pairs["transitions/config"]
	if err := alertsV2RulesFirstSeen("hs_avg", "hs_max", "hs_calc")(rules.doc[0]); err != nil {
		t.Errorf("the rules' guard refuses C's answer: %v", err)
	}
	if err := alertsV2RulesFirstSeen("hs_calc", "hs_max", "hs_avg")(rules.doc[0]); err == nil {
		t.Errorf("the rules' guard takes other names")
	}
	if err := alertsV2RulesFirstSeen("hs_avg", "hs_max", "hs_calc")(pairs["transitions/facets"].doc[0]); err == nil {
		t.Errorf("the rules' guard takes an answer without rules")
	}
	// rules without a name (`options=mcp`): their order alone
	nameless := pairs["transitions/config-mcp"]
	if err := alertsV2RulesFirstSeen()(nameless.doc[0]); err != nil {
		t.Errorf("the rules' guard refuses C's nameless rules: %v", err)
	}
	if err := alertsV2RulesFirstSeen("null", "null", "null")(nameless.doc[0]); err == nil {
		t.Errorf("the rules' guard reads a rule without a name as one named null")
	}
	three := regexp.MustCompile(`("configurations":\[)(\{"config_hash_id":"674523c2.*?\}),(\{"config_hash_id":"f090fc72.*?\}),` +
		`(\{"config_hash_id":"cbc27ceb.*?\})(\],"items")`)
	text := string(nameless.body[0])
	if !three.MatchString(text) {
		t.Fatalf("the recorded nameless rules are not the three, in the order the transitions show them: %s", text)
	}
	for name, order := range map[string]string{
		"the order they are stored in": "$1$4,$3,$2$5", "the last one twice": "$1$2,$3,$4,$4$5", "the first two": "$1$2,$3$5", "none": "$1$5",
	} {
		v, err := ParseJSON([]byte(three.ReplaceAllString(text, order)))
		if err != nil {
			t.Fatal(err)
		}
		if err := alertsV2RulesFirstSeen()(v); err == nil {
			t.Errorf("the rules' guard takes the rules as %s", name)
		}
	}
	// a template's rule has a null name, which the guard reads `null`
	if err := alertsV2RulesFirstSeen("hr_plain", "hr_two", "null")(pairs["rules/config"].doc[0]); err != nil {
		t.Errorf("the rules' guard refuses C's template: %v", err)
	}
	if err := alertsV2RulesFirstSeen("hr_plain", "hr_two", "hr_tpl")(pairs["rules/config"].doc[0]); err == nil {
		t.Errorf("the rules' guard takes a template by its name")
	}

	// a want as the agent writes it, against Value.String's escapes
	v, _ := ParseJSON([]byte(`{"a":"1 < 2 > 0 & so"}`))
	if err := alertsV2Is(`"1 < 2 > 0 & so"`, "a")(v); err != nil {
		t.Errorf("a want with the characters Value.String escapes: %v", err)
	}
	if err := alertsV2Is(`"1 < 2 > 1 & so"`, "a")(v); err == nil {
		t.Errorf("a want with the characters Value.String escapes holds for another text")
	}
	// the echo, as the rows want it
	if got, want := alertsV2Echo(`"debug"`, "null", `"n"`, `"last":1`, alertsV2NoFacets),
		`{"mode":["nodes","alert_transitions"],"options":["debug"],"scope":{"scope_nodes":null},"selectors":{"nodes":"n",`+
			`"alerts":{"last":1}},"filters":{"after":-3600,"before":0},"facets":{"f_status":null,"f_class":null,"f_type":null,`+
			`"f_component":null,"f_role":null,"f_node":null,"f_alert":null,"f_instance":null,"f_context":null}}`; got != want {
		t.Errorf("the echo is %s, want %s", got, want)
	}
	// the row of a window that ends some seconds ago: its `before` counts back from its clock to three seconds before
	// the third switch
	if got := alertsV2TransitionsAgoRow(1000, [4]int64{0, 0, 0, 990}).target; got != "/api/v2/alert_transitions?last=1&options=minify&before=-13" {
		t.Errorf("the window-ago row asks %s", got)
	}
	// the rows that name a transition: each side its own id, as the row spells it
	raised, first := [2]string{"aaaaaaaa-1111-4111-8111-00000000000a", "bbbbbbbb-1111-4111-8111-00000000000b"},
		[2]string{"cccccccc-1111-4111-8111-00000000000c", "dddddddd-1111-4111-8111-00000000000d"}
	spelled := map[string][2]string{}
	for _, row := range slices.Concat(alertsV2TransitionsOneRows(raised, first), alertsV2RulesOneRows(first)) {
		var ids [2]string
		for i := range ids {
			_, query, _ := strings.Cut(row.targetOf(i), "transition=")
			ids[i], _, _ = strings.Cut(query, "&")
		}
		spelled[row.name] = ids
	}
	if want := map[string][2]string{
		"one-bare":     {"AAAAAAAA11114111811100000000000A", "BBBBBBBB11114111811100000000000B"},
		"one-appended": {raised[0] + "zz", raised[1] + "zz"},
		"one-last2":    raised, "one-filters": raised, "one-facet": raised, "one-anchor": raised, "one-config": raised, "one-mcp": raised,
		"first-rfc3339": first, "idle": first,
	}; !maps.Equal(spelled, want) {
		t.Errorf("the rows that name a transition ask the ids %v, want %v", spelled, want)
	}
}

// dashNormTransitionsTimeout pins the timeout's rows: the recorded answers, and the round of such a row (v2Round)
// asked of two stub agents that write C's 504 with the Date of the second they answer in.
func dashNormTransitionsTimeout(t *testing.T, guards map[string]func(Value) error) {
	t.Helper()
	for _, row := range alertsV2TimeoutRows {
		if row.status != "504" || !strings.Contains(row.target, "timeout=-1") {
			t.Errorf("the row %s asks %s for a %s", row.name, row.target, row.status)
		}
		for i, a := range dashNormTransitionsRaw["transitions/"+row.name] {
			head, body, _ := strings.Cut(a, "\r\n\r\n")
			if !strings.HasPrefix(head, "HTTP/1.1 504 Gateway Timeout\r\n") ||
				!strings.Contains(head, "\r\nContent-Type: application/json; charset=utf-8\r\n") || !strings.Contains(head, "\r\nContent-Length: 13\r\n") {
				t.Errorf("%s: side %d's recorded answer is no 504 of the JSON type with its length: %q", row.name, i, head)
			}
			if err := guards["transitions/"+row.name](Value{Kind: KindString, Text: body}); err != nil {
				t.Errorf("%s: the guard refuses side %d's recorded body: %v", row.name, i, err)
			}
		}
	}
	answer := func(status, kind, body string) func() string {
		return func() string {
			now := time.Now().UTC().Format(http.TimeFormat)
			return "HTTP/1.1 " + status + "\r\nConnection: close\r\nServer: stub\r\nDate: " + now + "\r\nContent-Type: " + kind +
				"\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: " + now +
				fmt.Sprintf("\r\nContent-Length: %d\r\n", len(body)) + "X-Transaction-ID: 0123456789abcdef0123456789abcdef\r\n\r\n" + body
		}
	}
	const jsonType, timeout = "application/json; charset=utf-8", "504 Gateway Timeout"
	c504 := answer(timeout, jsonType, "query timeout")
	fam := alertsV2Family([2]*healthNorm{}, nil)
	for name, c := range map[string]struct {
		o, c func() string
		want string
	}{
		"C's answers":                       {c504, c504, ""},
		"a candidate of the text type":      {c504, answer(timeout, "text/plain; charset=utf-8", "query timeout"), "headers differ"},
		"a candidate that answers 200":      {c504, answer("200 OK", jsonType, "query timeout"), "headers differ"},
		"a candidate with another text":     {c504, answer(timeout, jsonType, "query timed out"), "bodies differ"},
		"a candidate that answers the JSON": {c504, answer("200 OK", jsonType, `{"api":2}`), "bodies differ"},
		"an oracle that answers 200":        {answer("200 OK", jsonType, "query timeout"), c504, "oracle: answered"},
		"an oracle with another text":       {answer(timeout, jsonType, "query interrupted"), c504, "oracle: body is not"},
	} {
		for _, row := range alertsV2TimeoutRows {
			pair := &Pair{Oracle: dashNormStub(t, c.o), Candidate: dashNormStub(t, c.c)}
			got := strings.Join(v2Round(pair, row, fam).problems, "\n")
			if (c.want == "") != (got == "") || !strings.Contains(got, c.want) {
				t.Errorf("the round of %s, %s: problems %q, want %q", row.name, name, got, c.want)
			}
		}
	}
}
