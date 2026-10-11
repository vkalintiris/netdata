// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The closer rows of milestone 10 on the alert endpoints' transitions side: the transitions over two hosts that
// D234 F3 decided (health.child `two-hosts`), the alerts rows of states no other fixture reaches (CRITICAL in
// `transitions`, a host whose health never ran beside one with alerts, a node selector that picks one of two hosts,
// UNDEFINED, REMOVED), the echo of a transition id, and the transitions' delay, flags, cuts and named chart (the
// `states` case).

// alertsV2EchoRe is the request's echo of `transition=` (`options=debug`: `selectors.alerts.transition`,
// database/contexts/api_v2_contexts.c:1416), the text the request sent.
var alertsV2EchoRe = regexp.MustCompile(`"transition":(\s*)"([^"\\]*)"`)

// alertsV2EchoAlias is body with the echo of a transition id written as its alias where the text is an id of the side's
// alert log as the log prints it (alertsV2Entry.named): each side echoes its own id. Any other text stays as the
// agent wrote it.
func alertsV2EchoAlias(n *healthNorm, log alertsV2Log, body []byte) []byte {
	return alertsV2EchoRe.ReplaceAllFunc(body, func(m []byte) []byte {
		g := alertsV2EchoRe.FindSubmatch(m)
		entry, known := log[string(g[2])]
		if !known {
			return m
		}
		return []byte(`"transition":` + string(g[1]) + `"` + entry.named(n) + `"`)
	})
}

// alertsV2NewestEntry is, per side, the id of the newest entry of the host's alert log (prefix: empty for the agent's
// own, `/host/<name>` for a child's, read with the host's normalizers n) of the alert `name`, whatever its statuses.
// The oracle must have one; a candidate without one is reported and gets an empty id.
func alertsV2NewestEntry(t *testing.T, h *healthPair, n [2]*healthNorm, prefix, name string) [2]string {
	t.Helper()
	var ids [2]string
	for i, side := range h.p.Each() {
		entries, err := h.entriesAs(n[i], i, prefix+"/api/v1/alarm_log")
		if err != nil && side.Role == Oracle {
			t.Fatalf("oracle: %v", err)
		}
		for _, e := range entries {
			if e.Name == name {
				ids[i] = e.Tid
			}
		}
		if ids[i] != "" {
			continue
		}
		if side.Role == Oracle {
			t.Fatalf("oracle: its alert log%s has no entry of %s", prefix, name)
		}
		t.Errorf("candidate: its alert log%s has no entry of %s", prefix, name)
	}
	return ids
}

// alertsV2WaitWindow sleeps until the newest entry of the oracle's alert logs at prefixes (empty: localhost's) is in
// a relative window: from its second + 2 on, one more for a side a second behind (alertsV2PlayTransitions).
func alertsV2WaitWindow(t *testing.T, h *healthPair, n [2]*healthNorm, prefixes ...string) {
	t.Helper()
	newest := int64(0)
	for _, prefix := range prefixes {
		entries, err := h.entriesAs(n[0], 0, prefix+"/api/v1/alarm_log")
		if err != nil {
			t.Fatalf("oracle: %v", err)
		}
		for _, e := range entries {
			newest = max(newest, e.When)
		}
	}
	time.Sleep(time.Until(time.Unix(newest+3, 0)))
}

// alertsV2ByID is a row of `/api/v3/alert_transitions` that names a transition, each side's own (ids), with the query
// after it.
func alertsV2ByID(name string, ids [2]string, query string, guard func(Value) error) v2Req {
	const one = "/api/v3/alert_transitions?transition="
	return v2Req{name: name, target: one + "<" + name + ">" + query, targets: [2]string{one + ids[0] + query, one + ids[1] + query},
		status: "200", guard: guard}
}

// alertsV2EchoAliased is a fact on an answer with `options=debug`: the echo of `transition=` reads as the alias of an
// entry of the answering host's alert log, `<prefix>t+<k>` (alertsV2EchoAlias; prefix: `<host>:` for another host's
// entry), and, where the answer has transitions, it is the first one's id as the render names it: each side echoes the
// id it was asked, the text of the request (database/contexts/api_v2_contexts.c:1416).
func alertsV2EchoAliased(prefix string) dashFact {
	alias := regexp.MustCompile(`^` + regexp.QuoteMeta(prefix) + `t\+\d+$`)
	return func(v Value) error {
		echo, err := dashAt(v, "request", "selectors", "alerts", "transition")
		if err != nil {
			return err
		}
		if echo.Kind != KindString || !alias.MatchString(echo.Text) {
			return fmt.Errorf("the echo's transition is %s, not an alias %st+k", echo, prefix)
		}
		if first, err := dashAt(v, "transitions", "[0]", "transition_id"); err == nil && first.Text != echo.Text {
			return fmt.Errorf("the echo's transition is %s, the transition's id %s", echo, first)
		}
		return nil
	}
}

// alertsV2HostIDs is a fact on a transitions answer over two hosts: each transition's id reads as its own host's alias,
// `health-child:t+k` for the child's (alertsV2Host), `t+k` for localhost's.
func alertsV2HostIDs(v Value) error {
	transitions, err := dashMember(v, "transitions")
	if err != nil {
		return err
	}
	for i, tr := range transitions.Items {
		host, err := dashMember(tr, "hostname")
		if err != nil {
			return fmt.Errorf("transitions[%d]: %v", i, err)
		}
		prefix := ""
		if host.Text == healthChild.Hostname {
			prefix = healthChild.Hostname + ":"
		}
		id, err := dashMember(tr, "transition_id")
		if err != nil {
			return fmt.Errorf("transitions[%d]: %v", i, err)
		}
		if !strings.HasPrefix(id.Text, prefix+"t+") {
			return fmt.Errorf("transitions[%d] of %s has the id %s, not its host's alias", i, host, id)
		}
	}
	return nil
}

// alertsV2OneOption is alertsV2Facets for an answer whose rows are n transitions of one alert of one host: each facet
// has one option, with the count n, its status and its alert as given, no class, type or component (`unknown`), the
// default recipient, the host's GUID named by its hostname.
func alertsV2OneOption(n int, status, guid, host, alert, instance, context string) func(Value) error {
	one := func(value string) string { return fmt.Sprintf("%s=%d", value, n) }
	return alertsV2Facets(map[string]string{"f_status": one(status), "f_class": one("unknown"), "f_type": one("unknown"),
		"f_component": one("unknown"), "f_role": one("root"), "f_node": one(guid + "(" + host + ")"), "f_alert": one(alert),
		"f_instance": one(instance), "f_context": one(context)})
}

// --- health.child `two-hosts` (health_child_test.go): the node selectors, and the transitions of two hosts

// alertsV2TwoHostsNodes are `two-hosts`' alerts rows with a node selector that picks one of the two hosts, asked
// beside alertsV2TwoHosts (each host's alert WARNING).
func alertsV2TwoHostsNodes(t *testing.T, h *healthPair, cn [2]*healthNorm) {
	t.Helper()
	fam := alertsV2Family(h.n, alertsV2LogReader(h),
		alertsV2Host{name: healthChild.Hostname, n: cn, log: alertsV2LogReaderOf(h, healthChildBase)})
	for _, req := range alertsV2TwoHostsNodeRows() {
		compareV2(t, h.p, req, fam)
	}
}

// alertsV2TwoHostsNodeRows are alertsV2TwoHostsNodes' rows. Their guards read what C answers: `nodes=` and
// `scope_nodes=` match a host by its hostname, then by its machine GUID (database/contexts/query_scope.c:36-60), and a
// negative word that matches a host leaves it out (`!health-child|*`: SP_MATCHED_NEGATIVE is no match, :45-47); the one
// host left is the first listed, so its index is 0 and its instance carries it (api_v2_contexts_alerts.c:718), and
// the groupings count its alert alone (the recipient's `available` counts the rules, both: :385-392).
func alertsV2TwoHostsNodeRows() []v2Req {
	at := func(query string) string { return alertsV2Route + "?" + alertsV2Full + "&" + query }
	host := func(nodes string, alert, chart, module string) func(Value) error {
		return alertsV2Guard(alertsV2Rows{member: "nodes", keys: []string{"nm", "ni"}, items: []string{nodes + " 0"}},
			alertsV2Rows{member: "alerts", keys: []string{"ati", "ni", "nm", "wr", "in", "nd"}, items: []string{"0 [0] " + alert + " 1 1 1"}},
			alertsV2Rows{member: "alert_instances", keys: []string{"ni", "nm", "ch", "st", "v"}, items: []string{"0 " + alert + " " + chart + " WARNING 70"}},
			alertsV2Rows{member: "alerts_by_recipient", keys: alertsV2RecipientKeys, items: []string{"root 0 1 0 1 0 2"}},
			alertsV2Rows{member: "alerts_by_module", keys: alertsV2ModuleKeys, items: []string{module + " 0 1 0 1 0"}})
	}
	child := host(healthChild.Hostname, "hch_calc", healthChildChart, "fanout")
	parent := host(parentIdentity.Hostname, "hloc_calc", healthTwoHostsChart, "[none]")
	return []v2Req{
		{name: "nodes-child", target: at("nodes=" + healthChild.Hostname), status: "200", guard: child},
		{name: "nodes-child-guid", target: at("nodes=" + healthChild.MachineGUID), status: "200", guard: child},
		{name: "scope-nodes-parent", target: at("scope_nodes=" + parentIdentity.Hostname), status: "200", guard: parent},
		{name: "nodes-not-child", target: at("nodes=!" + healthChild.Hostname + "%7C*"), status: "200", guard: parent},
	}
}

// alertsV2TwoHostsTransitions are `two-hosts`' transitions rows (D234 F3), asked at the case's end, once each host's
// newest change is in a relative window: each host's alert went CLEAR, WARNING, CLEAR, WARNING (both switched at one
// second, healthPair.atRelease), the child's last two changes while it was silenced.
func alertsV2TwoHostsTransitions(t *testing.T, h *healthPair, cn [2]*healthNorm) {
	t.Helper()
	alertsV2WaitWindow(t, h, h.n, "")
	alertsV2WaitWindow(t, h, cn, healthChildBase)
	fam := alertsV2Family(h.n, alertsV2LogReader(h),
		alertsV2Host{name: healthChild.Hostname, n: cn, log: alertsV2LogReaderOf(h, healthChildBase)})
	child := alertsV2NewestEntry(t, h, cn, healthChildBase, "hch_calc")
	for _, req := range alertsV2TwoHostsTransitionRows(child) {
		compareV2(t, h.p, req, fam)
	}
}

// alertsV2TwoHostsWindow are `two-hosts`' six transitions in the order every answer lists them, newest first (alert,
// host, statuses, new value, the notification's flags as a set). The order is the fixture's: the window's statement
// reads the rows of every host of its host table by global id (database/sqlite/sqlite_health.c:1516-1540, :1558), and
// HEALTH walks localhost before the child in a pass (health/health_event_loop.c:952-958: the host index's order), so of
// two changes made in one pass the child's is the newer; the child's value reaches the parent about 50 ms after
// localhost's (fanoutChild.run writes 50 ms into the second, the fake plugin at it), so a pass between the two changes
// localhost's alert first and the child's a pass later: newer again. One interleaving swaps a pair, by reading (never
// seen: four sides of two runs, C against C): localhost's plugin writing its block more than about 50 ms late, with a
// pass between the child's block and localhost's. Its signature: a difference at `transitions[2k]` and `[2k+1]` with
// one side's hloc_calc entry a pass after its hch_calc entry; no verdict on the candidate, run again. The child's last
// two entries were made while its alert was silenced: SILENCED, and no notification run (health/health_event_loop.c:
// 750, :853; health/health_notifications.c:441); an older entry its alert's next one updated is saved again with its
// notification's final flags, UPDATED, without EXEC_IN_PROGRESS, which the newest one, saved as its notification
// started, keeps (health/health_log.c:276-294).
var alertsV2TwoHostsWindow = func() []string {
	c, p := healthChild.Hostname+" "+healthChild.MachineGUID, parentIdentity.Hostname+" "+parentIdentity.MachineGUID
	return []string{
		"hch_calc " + c + " CLEAR WARNING 70 {PROCESSED,SAVED,SILENCED}",
		"hloc_calc " + p + " CLEAR WARNING 70 {EXEC_IN_PROGRESS,EXEC_RUN,PROCESSED,SAVED}",
		"hch_calc " + c + " WARNING CLEAR 10 {PROCESSED,SAVED,SILENCED,UPDATED}",
		"hloc_calc " + p + " WARNING CLEAR 10 {EXEC_RUN,PROCESSED,SAVED,UPDATED}",
		"hch_calc " + c + " CLEAR WARNING 70 {EXEC_RUN,PROCESSED,SAVED,UPDATED}",
		"hloc_calc " + p + " CLEAR WARNING 70 {EXEC_RUN,PROCESSED,SAVED,UPDATED}",
	}
}()

// alertsV2TwoHostsKeys are the members of a transition alertsV2TwoHostsWindow reads.
var alertsV2TwoHostsKeys = []string{"alert", "hostname", "machine_guid", "old.status", "new.status", "new.value", "notification.flags{}"}

// alertsV2TwoHostsTransitionRows are alertsV2TwoHostsTransitions' rows; child is each side's id of the child's
// newest entry. Their guards read what C answers:
//   - the window's host table holds both hosts, so their six rows are evaluated (alertsV2TwoHostsWindow); `f_node`
//     has both hosts' GUIDs, each named by its hostname (api_v2_contexts_alert_transitions.c:369-380), in the order
//     the rows show them (:276-283), and each transition its host's hostname (:416-419) and its id from its own
//     host's alert log (alertsV2HostIDs);
//   - `f_node=` of the child's GUID matches its rows; localhost's are evaluated and counted on that facet alone
//     (:285-322);
//   - `nodes=` of the child leaves localhost out of the host table: three rows evaluated, one host in `f_node`
//     (database/contexts/api_v2_contexts.c:658-674);
//   - `options=mcp`: no id and no GUID, the hostname kept; the render names each transition by its change in its own
//     host's log (alertsV2Log.changed);
//   - by the id of the child's newest entry, with `debug`: the direct statement reads any host's row
//     (sqlite_health.c:1476-1479), and the echo is the child's alias (alertsV2EchoAliased).
func alertsV2TwoHostsTransitionRows(child [2]string) []v2Req {
	const t2 = "/api/v2/alert_transitions?after=-600&last=200&options=minify"
	w := alertsV2TwoHostsWindow
	cg, pg := healthChild.MachineGUID+"("+healthChild.Hostname+")", parentIdentity.MachineGUID+"("+parentIdentity.Hostname+")"
	facets := func(status string, n int, nodes, alerts, instances, contexts string) func(Value) error {
		return alertsV2Facets(map[string]string{"f_status": status, "f_class": fmt.Sprintf("unknown=%d", n),
			"f_type": fmt.Sprintf("unknown=%d", n), "f_component": fmt.Sprintf("unknown=%d", n), "f_role": fmt.Sprintf("root=%d", n),
			"f_node": nodes, "f_alert": alerts, "f_instance": instances, "f_context": contexts})
	}
	rows := func(items string, at ...int) dashFact {
		changes := []string{}
		for _, i := range at {
			changes = append(changes, w[i])
		}
		return alertsV2Guard(alertsV2Items(items), alertsV2Rows{member: "transitions", keys: alertsV2TwoHostsKeys, items: changes})
	}
	mcp := []string{}
	for _, line := range w {
		f := strings.Fields(line)
		mcp = append(mcp, "G "+f[0]+" "+f[1]+" "+f[3]+" "+f[4]+" WHEN")
	}
	return []v2Req{
		{name: "transitions", target: t2, status: "200", guard: dashGuard([]dashFact{rows("6 6 6 200 0 0", 0, 1, 2, 3, 4, 5),
			facets("WARNING=4 CLEAR=2", 6, cg+"=3 "+pg+"=3", "hch_calc=3 hloc_calc=3", healthChildChart+"=3 "+healthTwoHostsChart+"=3",
				healthChildContext+"=3 hloc.ctx=3"), alertsV2HostIDs})},
		{name: "transitions-f-node", target: t2 + "&f_node=" + healthChild.MachineGUID, status: "200", guard: dashGuard([]dashFact{
			rows("6 3 3 200 0 0", 0, 2, 4), facets("WARNING=2 CLEAR=1", 3, cg+"=3 "+pg+"=3", "hch_calc=3 hloc_calc=0",
				healthChildChart+"=3 "+healthTwoHostsChart+"=0", healthChildContext+"=3 hloc.ctx=0"), alertsV2HostIDs})},
		{name: "transitions-nodes", target: t2 + "&nodes=" + healthChild.Hostname, status: "200", guard: dashGuard([]dashFact{
			rows("3 3 3 200 0 0", 0, 2, 4), facets("WARNING=2 CLEAR=1", 3, cg+"=3", "hch_calc=3", healthChildChart+"=3",
				healthChildContext+"=3"), alertsV2HostIDs})},
		{name: "transitions-mcp", target: "/api/v2/alert_transitions?after=-600&last=200&options=mcp,minify", status: "200",
			guard: dashGuard([]dashFact{dashKeys("transitions items"), dashKeys(alertsV2McpKeys, "transitions", "[0]"),
				alertsV2Guard(alertsV2Items("6 6 6 200 0 0"), alertsV2Rows{member: "transitions",
					keys: []string{"gi", "alert", "hostname", "old.status", "new.status", "when"}, items: mcp})})},
		alertsV2ByID("transition-child-debug", child, "&options=minify,debug", dashGuard([]dashFact{
			alertsV2Guard(alertsV2Items("1 1 1 1 0 0"), alertsV2Rows{member: "transitions", keys: alertsV2TwoHostsKeys, items: w[:1]}),
			alertsV2OneOption(1, "WARNING", healthChild.MachineGUID, healthChild.Hostname, "hch_calc", healthChildChart, healthChildContext),
			alertsV2EchoAliased(healthChild.Hostname + ":")})),
	}
}

// --- api.v2-alerts' `transitions` case: the alerts rows while its three alerts are CRITICAL, and the echo

// alertsV2TransitionsCritical are the `transitions` case's alerts rows at its third phase (alertsV2PlayTransitions),
// once its three alerts are CRITICAL.
func alertsV2TransitionsCritical(t *testing.T, h *healthPair) {
	t.Helper()
	fam := alertsV2Family(h.n, alertsV2LogReader(h))
	for _, req := range alertsV2TransitionsCriticalRows() {
		compareV2(t, h.p, req, fam)
	}
}

// alertsV2TransitionsCriticalRows are alertsV2TransitionsCritical's rows. Their guards read what C answers for
// hs_calc, hs_max and hs_avg CRITICAL at 95 (the alerts in the rules' order):
//   - an alert counts in `cr` (api_v2_contexts_alerts.c:183-208), and so does its grouping entry;
//   - `status=critical` and `raised` keep the three (:70-99: `raised` is WARNING or above), `warning` none; a status
//     word with no alert of the host's last pass skips the host's walk and lists the host (database/contexts/
//     api_v2_contexts.c:648-661), under a window too, through its retention (:675-683); `critical` under a window walks
//     it and keeps the three.
func alertsV2TransitionsCriticalRows() []v2Req {
	at := func(query string) string { return alertsV2Route + "?" + query }
	names := alertsV2Rows{member: "alerts", keys: []string{"nm", "cr", "wr", "cl", "er", "in"},
		items: []string{"hs_calc 1 0 0 0 1", "hs_max 1 0 0 0 1", "hs_avg 1 0 0 0 1"}}
	instances := alertsV2Rows{member: "alert_instances", keys: []string{"nm", "st"},
		items: []string{"hs_calc CRITICAL", "hs_max CRITICAL", "hs_avg CRITICAL"}}
	counted := []alertsV2Rows{alertsV2Nodes(alertsV2Parent),
		{member: "alerts_by_recipient", keys: alertsV2RecipientKeys, items: []string{"root 3 0 0 3 0 3"}},
		{member: "alerts_by_module", keys: alertsV2ModuleKeys, items: []string{"[none] 3 0 0 3 0"}}}
	none := []alertsV2Rows{alertsV2Nodes(alertsV2Parent), {member: "alerts", keys: []string{"nm"}, items: []string{}},
		{member: "alerts_by_recipient", keys: alertsV2RecipientKeys, items: []string{"root 0 0 0 0 0 3"}},
		{member: "alerts_by_module", keys: alertsV2ModuleKeys, items: []string{}}}
	return []v2Req{
		{name: "alerts-critical", target: at(alertsV2Full + "&status=critical"), status: "200", guard: alertsV2Guard(append(slices.Clone(counted),
			names, alertsV2Rows{member: "alert_instances", keys: []string{"nm", "st", "v", "tr_v"},
				items: []string{"hs_calc CRITICAL 95 95", "hs_max CRITICAL 95 95", "hs_avg CRITICAL 95 95"}})...)},
		{name: "alerts-warning", target: at("options=summary,instances,minify&status=warning"), status: "200",
			guard: alertsV2Guard(append(slices.Clone(none), alertsV2Rows{member: "alert_instances", keys: []string{"nm"}, items: []string{}})...)},
		{name: "alerts-raised", target: at("options=summary,instances,minify&status=raised"), status: "200",
			guard: alertsV2Guard(append(slices.Clone(counted), names, instances)...)},
		{name: "alerts-summary", target: at("options=summary,minify"), status: "200",
			guard: alertsV2Guard(append(slices.Clone(counted), names, alertsV2Rows{member: "alert_instances"})...)},
		{name: "alerts-warning-window", target: at("options=summary,minify&status=warning&after=-600"), status: "200",
			guard: alertsV2Guard(none...)},
		{name: "alerts-critical-window", target: at("options=summary,instances,minify&status=critical&after=-600"), status: "200",
			guard: alertsV2Guard(append(slices.Clone(counted), names, instances)...)},
	}
}

// alertsV2TransitionsEchoRows are the `transitions` case's rows of `debug` beside `transition=` (raised: each side's
// id of hs_calc's change to WARNING), one per mode: each side echoes its own id (database/contexts/api_v2_contexts.c:
// 1416), which the render names (alertsV2EchoAliased). The alerts mode keeps the transition's alert, hs_calc, as it is
// now (CLEAR, the case's last phase: alertsV2TransitionRows); the transitions mode its one row.
func alertsV2TransitionsEchoRows(raised [2]string) []v2Req {
	return []v2Req{
		alertsV2TransitionRow("echo-alerts", raised, "&"+alertsV2Full+",debug", dashGuard([]dashFact{
			alertsV2Guard(alertsV2Nodes(alertsV2Parent), alertsV2Rows{member: "alerts", keys: []string{"nm", "cl", "in"}, items: []string{"hs_calc 1 1"}},
				alertsV2Rows{member: "alert_instances", keys: []string{"nm", "st", "v"}, items: []string{"hs_calc CLEAR 10"}}),
			dashKeys("status alert transition", "request", "selectors", "alerts"), alertsV2EchoAliased("")})),
		alertsV2ByID("echo-transitions", raised, "&options=minify,debug", dashGuard([]dashFact{
			alertsV2Guard(alertsV2Items("1 1 1 1 0 0"), alertsV2Changes("hs_calc CLEAR WARNING 70")),
			alertsV2SigFacets("WARNING=1", "hs_calc=1", 1, nil),
			dashKeys("context anchor_gi last alert transition", "request", "selectors", "alerts"), alertsV2EchoAliased("")})),
	}
}

// --- api.v2-alerts' `states` case

// alertsV2StatesLong are the `states` case's long texts: the name of an alert, its units, type and component, its
// recipient, its exec and summary, and the context of its chart, each longer than the field C keeps it in for a
// returned transition (api_v2_contexts_alert_transitions.c:71-111: 47 bytes, 95 and 511), the units and the summary
// with a character of two bytes across the cut.
var (
	alertsV2StatesLongName    = "hst_long_" + strings.Repeat("n", 43)
	alertsV2StatesLongUnits   = strings.Repeat("u", 46) + "ézz"
	alertsV2StatesLongType    = "Type " + strings.Repeat("t", 55)
	alertsV2StatesLongPart    = "Part " + strings.Repeat("p", 55)
	alertsV2StatesLongTo      = strings.Repeat("r", 100)
	alertsV2StatesLongExec    = "/dev/null/" + strings.Repeat("x", 200) + "/" + strings.Repeat("y", 200) + "/" + strings.Repeat("z", 200)
	alertsV2StatesLongSummary = strings.Repeat("s", 510) + "ézz"
	alertsV2StatesLongContext = "hst." + strings.Repeat("c", 96)
)

// alertsV2StatesConf are the `states` case's rules (alertsV2StatesScenario):
//   - on the collected chart hst.values, `calc: $a` through 10, 70 and 10: hst_delay, notified 10 s after it rises;
//     hst_fail, whose notifier is a path nothing can be executed under; hst_nan, whose value is a variable no chart
//     has and whose warning is that value (NaN: UNDEFINED); hst_undef, whose warning divides by zero (UNDEFINED with
//     a value that is a number);
//   - hst_named on hst.idn, a chart whose name is not its id, collected once at each phase's start;
//   - hst_gone on hst.gone, never collected and made obsolete at the second phase's start;
//   - hst_idle and the long alert (alertsV2StatesLong) on hst.idle, never collected, a chart without units of a long
//     context: the first without units, the second with every long text.
var alertsV2StatesConf = ` alarm: hst_delay
    on: hst.values
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
 delay: up 10s
  info: notified 10 s after it rose

 alarm: hst_fail
    on: hst.values
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  exec: /dev/null/hst-notifier
  info: its notifier cannot run

 alarm: hst_nan
    on: hst.values
  calc: $hst_nothing
 every: 1s
  warn: $this
 units: things
  info: its value is not a number

 alarm: hst_undef
    on: hst.values
  calc: $a
 every: 1s
  warn: $this / 0
 units: things
  info: its warning cannot be evaluated

 alarm: hst_named
    on: hst.idn
  calc: $b
 every: 1s
  warn: $this > 50
 units: things
  info: on a chart with a name of its own

 alarm: hst_gone
    on: hst.gone
  calc: $b
 every: 1s
  warn: $this > 1000
 units: things
  info: on a chart made obsolete

 alarm: hst_idle
    on: hst.idle
  calc: $b
 every: 1s
  warn: $this > 1000
  info: no units on a chart without units

 alarm: ` + alertsV2StatesLongName + `
    on: hst.idle
  calc: $b
 every: 1s
  warn: $this > 1000
 units: ` + alertsV2StatesLongUnits + `
  type: ` + alertsV2StatesLongType + `
component: ` + alertsV2StatesLongPart + `
    to: ` + alertsV2StatesLongTo + `
  exec: ` + alertsV2StatesLongExec + `
summary: ` + alertsV2StatesLongSummary + `
  info: long texts
`

// alertsV2StatesScenario is the `states` case's plugin: hst.idn (named `named`), hst.gone and hst.idle (no units, the
// long context), then the collected hst.values through 10, 70 and 10. hst.idn gets one block at the first two
// phases' first seconds (10, then 70), and the second phase's start makes hst.gone obsolete.
func alertsV2StatesScenario() *plugin.Scenario {
	emit := "CHART hst.idn 'named' 'title' 'units' 'fam' 'hst.nctx' line 1000 1 '' '' ''\n" +
		"DIMENSION b '' absolute 1 1\n" +
		"CHART hst.gone '' 'title' 'units' 'fam' 'hst.gctx' line 1000 1 '' '' ''\n" +
		"DIMENSION b '' absolute 1 1\n" +
		"CHART hst.idle '' 'title' '' 'fam' '" + alertsV2StatesLongContext + "' line 1000 1 '' '' ''\n" +
		"DIMENSION b '' absolute 1 1\n"
	sc := healthScenario(emit, "hst.values", "hst.ctx", []string{"a"}, map[string]int64{"a": 10}, map[string]int64{"a": 70},
		map[string]int64{"a": 10})
	v := healthChart(sc)
	v.Phases[0].Emit = "BEGIN hst.idn\nSET b = 10\nEND {{sec}} 0\n"
	v.Phases[1].Emit = "BEGIN hst.idn\nSET b = 70\nEND {{sec}} 0\n" +
		"CHART hst.gone '' 'title' 'units' 'fam' 'hst.gctx' line 1000 1 'obsolete' '' ''\n"
	return sc
}

// alertsV2StatusOf is a guard on /api/v1/alarms?all: the named alerts have their statuses (others may show).
func alertsV2StatusOf(want map[string]string) func(string) error {
	return func(view string) error {
		got, err := healthStatuses(view)
		if err != nil {
			return err
		}
		for name, status := range want {
			if got[name] != status {
				return fmt.Errorf("%s is %q, want %s (all: %v)", name, got[name], status, got)
			}
		}
		return nil
	}
}

// alertsV2PlayStates plays `states`: hst.values at 10, then at 70; once hst_delay's notification ran after its delay
// and hst_fail's failed, back at 10, which stores hst_fail's change to WARNING again with what its notification
// came to (an entry that a newer one of its alert updates is saved again, health/health_log.c:276-294); then the
// rows.
func alertsV2PlayStates(t *testing.T, h *healthPair) {
	h.create(t)
	alarms := func(i int) string { return h.get(i, "/api/v1/alarms?all") }
	h.waitOracle(t, "the chart's alerts", func() (string, error) {
		v := alarms(0)
		return v, alertsV2StatusOf(map[string]string{"hst_delay": "CLEAR", "hst_fail": "CLEAR"})(v)
	})
	h.release(t, "p1", 1, healthCalcHold)
	// the alerts of the charts collected (hst.idn's after its second block): /api/v1/alarms?all lists no other
	raised := map[string]string{"hst_named": "WARNING", "hst_delay": "WARNING", "hst_fail": "WARNING", "hst_nan": "UNDEFINED",
		"hst_undef": "UNDEFINED"}
	cleared := maps.Clone(raised)
	cleared["hst_delay"], cleared["hst_fail"] = "CLEAR", "CLEAR"
	h.compareNow(t, "/api/v1/alarms?all", alarms, healthWant(raised))
	h.processed(t, "the delayed notification", "hst_delay", "WARNING")
	h.processed(t, "the notification that cannot run", "hst_fail", "WARNING")
	h.release(t, "p2", 2, healthCalcHold)
	h.compareNow(t, "back at 10: /api/v1/alarms?all", alarms, healthWant(cleared))
	h.processed(t, "back at 10", "hst_delay", "CLEAR")
	h.processed(t, "back at 10", "hst_fail", "CLEAR")
	// hst_fail's change to WARNING saved again, with its notification's failure
	h.waitOracle(t, "the failed notification stored", func() (string, error) {
		entries, err := h.entriesAs(h.n[0], 0, "/api/v1/alarm_log")
		if err != nil {
			return "", err
		}
		for _, e := range entries {
			if e.Name == "hst_fail" && e.OldStatus == "CLEAR" && e.Status == "WARNING" && e.ExecFailed {
				return "", nil
			}
		}
		return "", fmt.Errorf("no entry of hst_fail's change to WARNING has exec_failed")
	})
	h.compareNow(t, "the alert log's transitions", func(i int) string { return h.transitions(i, "") },
		alertsV2LogHolds("hst_delay: CLEAR->WARNING 70 things", "hst_fail: WARNING->CLEAR 10 things", "hst_gone: UNINITIALIZED->REMOVED -",
			"hst_named: UNINITIALIZED->WARNING 70 things", "hst_nan: UNINITIALIZED->UNDEFINED -", "hst_undef: UNINITIALIZED->UNDEFINED 10 things"))
	alertsV2WaitWindow(t, h, h.n, "")
	fam := alertsV2Family(h.n, alertsV2LogReader(h))
	for _, req := range alertsV2StatesRows() {
		compareV2(t, h.p, req, fam)
	}
	ids := map[string][2]string{}
	for _, name := range []string{"hst_nan", "hst_undef", "hst_named", "hst_idle", alertsV2StatesLongName, "hst_gone"} {
		ids[name] = alertsV2NewestEntry(t, h, h.n, "", name)
	}
	ids["hst_delay"] = alertsV2TransitionOf(t, h, h.n, "", "hst_delay", "CLEAR", "WARNING")
	ids["hst_fail"] = alertsV2TransitionOf(t, h, h.n, "", "hst_fail", "CLEAR", "WARNING")
	for _, req := range alertsV2StatesOneRows(ids) {
		compareV2(t, h.p, req, fam)
	}
}

// alertsV2StatesKeys are the members of a transition the `states` case's guards read, and alertsV2StatesWindow its
// window's five rows as they read, newest first: the changes of one pass in the order HEALTH walks the host's alerts,
// the order they were linked in, their charts' (hst.idn's hst_named first, then hst.values' in the rules' order:
// alertsV2Window), the later the newer. A transition's notification: hst_delay's after its delay (`delay` 10, its
// `when` and `delay_up_to_time` the change's second plus 10: health/health_event_loop.c:736, health/health_log.c:
// 251-252), hst_fail's run and failed
// (EXEC_FAILED and the shell's 127, health/health_notifications.c:76-79); both are on the record once a newer entry
// updated theirs, which saves it again (health/health_log.c:276-294); the newest entries keep the flags they were
// saved with as their notifications started (EXEC_IN_PROGRESS). The chart's name is `instance_n` and the id
// `instance` (api_v2_contexts_alert_transitions.c:419-423).
var (
	alertsV2StatesKeys   = []string{"alert", "old.status", "new.status", "new.value", "instance", "instance_n", "notification.delay", "notification.flags{}", "notification.exec_code"}
	alertsV2StatesWindow = []string{
		"hst_fail WARNING CLEAR 10 hst.values hst.values 0 {EXEC_IN_PROGRESS,EXEC_RUN,PROCESSED,SAVED} 0",
		"hst_delay WARNING CLEAR 10 hst.values hst.values 0 {EXEC_IN_PROGRESS,EXEC_RUN,PROCESSED,SAVED} 0",
		"hst_fail CLEAR WARNING 70 hst.values hst.values 0 {EXEC_FAILED,EXEC_RUN,PROCESSED,SAVED,UPDATED} 127",
		"hst_delay CLEAR WARNING 70 hst.values hst.values 10 {EXEC_RUN,PROCESSED,SAVED,UPDATED} 0",
		"hst_named UNINITIALIZED WARNING 70 hst.idn hst.named 0 {EXEC_IN_PROGRESS,EXEC_RUN,PROCESSED,SAVED} 0",
	}
)

// alertsV2StatesFacets is alertsV2Facets for an answer over the `states` case's window: `status`, `alerts`,
// `instances` and `contexts` the options of those facets (in the order the rows show them), the others one value each
// with the count `n`.
func alertsV2StatesFacets(status string, n int, alerts, instances, contexts string) func(Value) error {
	one := func(value string) string { return fmt.Sprintf("%s=%d", value, n) }
	return alertsV2Facets(map[string]string{"f_status": status, "f_class": one("unknown"), "f_type": one("unknown"),
		"f_component": one("unknown"), "f_role": one("root"), "f_node": one(parentIdentity.MachineGUID + "(" + parentIdentity.Hostname + ")"),
		"f_alert": alerts, "f_instance": instances, "f_context": contexts})
}

// alertsV2StatesRows are the `states` case's rows that ask both sides one target. Their guards read what C answers
// (alertsV2StatesWindow has the transitions'):
//   - `f_instance` selects by the chart's name, never its id (api_v2_contexts_alert_transitions.c:272): `hst.named`
//     keeps hst_named's row, `hst.idn` nothing; the MCP form writes the name as `instance` (:423);
//   - an alert's counts (api_v2_contexts_alerts.c:183-208): REMOVED and UNINITIALIZED in no column, UNDEFINED in `er`
//     only while its value is no number (hst_nan; hst_undef, UNDEFINED at 10, in none); the REMOVED alert of the
//     obsolete chart is still met and listed, its value null (health/health_event_loop.c:487-523);
//   - `status=undefined` keeps the two UNDEFINED alerts, under a window too (the host's last pass counted them,
//     database/contexts/api_v2_contexts.c:648-661); `raised` hst_named alone;
//   - an instance's `ch` is the chart's id, `ch_n` its name; the units of hst_idle, a rule without units on a chart
//     with none, are empty.
func alertsV2StatesRows() []v2Req {
	const w = "/api/v3/alert_transitions?after=-600&last=200&options=minify"
	at := func(query string) string { return alertsV2Route + "?" + query }
	sw := alertsV2StatesWindow
	window := func(rows ...string) alertsV2Rows {
		return alertsV2Rows{member: "transitions", keys: alertsV2StatesKeys, items: append([]string{}, rows...)}
	}
	grouping := []string{"name", "cr", "wr", "cl", "er", "running", "running_silent", "available?"}
	summary := []string{"hst_named 0 1 0 0 1", "hst_gone 0 0 0 0 1", "hst_idle 0 0 0 0 1", alertsV2StatesLongName + " 0 0 0 0 1",
		"hst_delay 0 0 1 0 1", "hst_fail 0 0 1 0 1", "hst_nan 0 0 0 1 1", "hst_undef 0 0 0 0 1"}
	undefined := []string{"hst_nan 0 0 0 1 1", "hst_undef 0 0 0 0 1"}
	alerts := func(items ...string) alertsV2Rows {
		return alertsV2Rows{member: "alerts", keys: []string{"nm", "cr", "wr", "cl", "er", "in"}, items: items}
	}
	return []v2Req{
		{name: "window", target: w, status: "200", guard: dashGuard([]dashFact{
			alertsV2Guard(alertsV2Items("5 5 5 200 0 0"), window(sw...)),
			alertsV2StatesFacets("CLEAR=2 WARNING=3", 5, "hst_fail=2 hst_delay=2 hst_named=1", "hst.values=4 hst.named=1", "hst.ctx=4 hst.nctx=1")})},
		{name: "instance-name", target: w + "&f_instance=hst.named", status: "200", guard: dashGuard([]dashFact{
			alertsV2Guard(alertsV2Items("5 1 1 200 0 0"), window(sw[4])),
			alertsV2StatesFacets("CLEAR=0 WARNING=1", 1, "hst_fail=0 hst_delay=0 hst_named=1", "hst.values=4 hst.named=1", "hst.ctx=0 hst.nctx=1")})},
		{name: "instance-id", target: w + "&f_instance=hst.idn", status: "200", guard: dashGuard([]dashFact{
			alertsV2Guard(alertsV2Items("5 0 0 200 0 0"), window()),
			alertsV2StatesFacets("CLEAR=0 WARNING=0", 0, "hst_fail=0 hst_delay=0 hst_named=0", "hst.values=4 hst.named=1", "hst.ctx=0 hst.nctx=0")})},
		{name: "mcp", target: "/api/v3/alert_transitions?after=-600&last=200&options=mcp,minify", status: "200", guard: dashGuard([]dashFact{
			dashKeys("transitions items"), dashKeys(alertsV2McpKeys, "transitions", "[0]"),
			alertsV2Guard(alertsV2Items("5 5 5 200 0 0"), alertsV2Rows{member: "transitions", keys: []string{"alert", "instance", "notification.exec_code"},
				items: []string{"hst_fail hst.values 0", "hst_delay hst.values 0", "hst_fail hst.values 127", "hst_delay hst.values 0", "hst_named hst.named 0"}})})},
		{name: "alerts", target: at(alertsV2Full), status: "200", guard: alertsV2Guard(alertsV2Nodes(alertsV2Parent), alerts(summary...),
			alertsV2Rows{member: "alert_instances", keys: []string{"nm", "ch", "ch_n", "st", "v", "units"}, items: []string{
				"hst_named hst.idn hst.named WARNING 70 things", "hst_gone hst.gone hst.gone REMOVED null things",
				"hst_idle hst.idle hst.idle UNINITIALIZED null ", alertsV2StatesLongName + " hst.idle hst.idle UNINITIALIZED null " + alertsV2StatesLongUnits,
				"hst_delay hst.values hst.values CLEAR 10 things", "hst_fail hst.values hst.values CLEAR 10 things",
				"hst_nan hst.values hst.values UNDEFINED null things", "hst_undef hst.values hst.values UNDEFINED 10 things"}},
			alertsV2Rows{member: "alerts_by_recipient", keys: grouping, items: []string{"root 0 1 2 1 7 0 7", alertsV2StatesLongTo + " 0 0 0 0 1 0 1"}},
			alertsV2Rows{member: "alerts_by_module", keys: grouping, items: []string{"[none] 0 1 2 1 8 0 -"}})},
		{name: "alerts-undefined", target: at("options=summary,instances,values,minify&status=undefined"), status: "200",
			guard: alertsV2Guard(alertsV2Nodes(alertsV2Parent), alerts(undefined...),
				alertsV2Rows{member: "alert_instances", keys: []string{"nm", "st", "v"}, items: []string{"hst_nan UNDEFINED null", "hst_undef UNDEFINED 10"}},
				alertsV2Rows{member: "alerts_by_module", keys: grouping, items: []string{"[none] 0 0 0 1 2 0 -"}})},
		{name: "alerts-undefined-window", target: at("options=summary,minify&status=undefined&after=-600"), status: "200",
			guard: alertsV2Guard(alertsV2Nodes(alertsV2Parent), alerts(undefined...), alertsV2Rows{member: "alert_instances"})},
		{name: "alerts-raised", target: at("options=summary,instances,minify&status=raised"), status: "200",
			guard: alertsV2Guard(alertsV2Nodes(alertsV2Parent), alerts("hst_named 0 1 0 0 1"),
				alertsV2Rows{member: "alert_instances", keys: []string{"nm", "ch", "ch_n", "st"}, items: []string{"hst_named hst.idn hst.named WARNING"}})},
		{name: "alerts-mcp", target: at("options=mcp,instances,values"), status: "200", guard: alertsV2Guard(alertsV2Rows{member: "alert_instances",
			keys: []string{"[0]", "[3]", "[4]", "[8]", "[10]", "[18]"}, items: []string{
				"hst_named hst.named WARNING things 70 70", "hst_gone hst.gone REMOVED things null null", "hst_idle hst.idle UNINITIALIZED  null null",
				alertsV2StatesLongName + " hst.idle UNINITIALIZED " + alertsV2StatesLongUnits + " null null", "hst_delay hst.values CLEAR things 10 10",
				"hst_fail hst.values CLEAR things 10 10", "hst_nan hst.values UNDEFINED things null null", "hst_undef hst.values UNDEFINED things 10 10"}})},
	}
}

// alertsV2StatesOneRows are the `states` case's rows of one transition each (ids: each side's, by the alert's name:
// hst_delay's and hst_fail's changes to WARNING, the newest entry of each other alert). Their guards read what C
// answers:
//   - hst_delay's notification after its delay: `delay` 10, its second and the delay's end the change's plus 10, each
//     its entry's (the render's marks); hst_fail's failure;
//   - an UNDEFINED change: hst_nan's value, NaN, is stored NULL and read as 0 (sqlite_health.c:1618:
//     sqlite3_column_double), hst_undef's is its number;
//   - the chart's id and name, the MCP form's name; the REMOVED of the obsolete chart, its value read as 0;
//   - hst_idle's units null: no units in its rule nor its chart (api_v2_contexts_alert_transitions.c:434);
//   - the long alert's texts cut as C keeps a returned transition (:71-111: 47 bytes of a name, units, type and
//     component, 95 of a context and a recipient, 511 of an exec and a summary; the units and the summary inside a
//     character, written out by the render, alertsV2Bytes); its info whole.
func alertsV2StatesOneRows(ids map[string][2]string) []v2Req {
	const q = "&options=minify"
	one := func(status, alert, instance, context string) func(Value) error {
		return alertsV2OneOption(1, status, parentIdentity.MachineGUID, parentIdentity.Hostname, alert, instance, context)
	}
	change := func(keys []string, item string) dashFact {
		return alertsV2Guard(alertsV2Items("1 1 1 1 0 0"), alertsV2Rows{member: "transitions", keys: keys, items: []string{item}})
	}
	base := []string{"alert", "old.status", "new.status", "new.value"}
	cut := func(s string, n int) string { return s[:n] }
	long := strings.Join([]string{cut(alertsV2StatesLongName, 47), cut(alertsV2StatesLongUnits, 46) + string(alertsV2Marker) + "c3",
		cut(alertsV2StatesLongType, 47), cut(alertsV2StatesLongPart, 47), cut(alertsV2StatesLongContext, 95),
		cut(alertsV2StatesLongTo, 95), cut(alertsV2StatesLongExec, 511), strings.Repeat("s", 510) + string(alertsV2Marker) + "c3", "long texts"}, " ")
	return []v2Req{
		alertsV2ByID("delay", ids["hst_delay"], q, dashGuard([]dashFact{
			change(append(slices.Clone(base), "notification.delay", "notification.flags{}", "when", "notification.when", "notification.delay_up_to_time"),
				"hst_delay CLEAR WARNING 70 10 {EXEC_RUN,PROCESSED,SAVED,UPDATED} WHEN EXEC_RUN DELAY_UP_TO"),
			one("WARNING", "hst_delay", "hst.values", "hst.ctx")})),
		alertsV2ByID("failed", ids["hst_fail"], q, dashGuard([]dashFact{
			change(append(slices.Clone(base), "notification.flags{}", "notification.exec_code", "notification.exec"),
				"hst_fail CLEAR WARNING 70 {EXEC_FAILED,EXEC_RUN,PROCESSED,SAVED,UPDATED} 127 /dev/null/hst-notifier"),
			one("WARNING", "hst_fail", "hst.values", "hst.ctx")})),
		alertsV2ByID("nan", ids["hst_nan"], q, dashGuard([]dashFact{change(base, "hst_nan UNINITIALIZED UNDEFINED 0"),
			one("UNDEFINED", "hst_nan", "hst.values", "hst.ctx")})),
		alertsV2ByID("undef", ids["hst_undef"], q, dashGuard([]dashFact{change(base, "hst_undef UNINITIALIZED UNDEFINED 10"),
			one("UNDEFINED", "hst_undef", "hst.values", "hst.ctx")})),
		alertsV2ByID("named", ids["hst_named"], q, dashGuard([]dashFact{change(append(slices.Clone(base), "instance", "instance_n"),
			"hst_named UNINITIALIZED WARNING 70 hst.idn hst.named"), one("WARNING", "hst_named", "hst.named", "hst.nctx")})),
		alertsV2ByID("named-mcp", ids["hst_named"], "&options=mcp,minify", dashGuard([]dashFact{dashKeys(alertsV2McpKeys, "transitions", "[0]"),
			change(append(slices.Clone(base), "instance"), "hst_named UNINITIALIZED WARNING 70 hst.named"), alertsV2Facets(nil)})),
		alertsV2ByID("idle", ids["hst_idle"], q, dashGuard([]dashFact{change(append(slices.Clone(base), "units", "context"),
			"hst_idle REMOVED UNINITIALIZED 0 null "+cut(alertsV2StatesLongContext, 95)),
			one("UNINITIALIZED", "hst_idle", "hst.idle", alertsV2StatesLongContext)})),
		alertsV2ByID("long", ids[alertsV2StatesLongName], q, dashGuard([]dashFact{
			change([]string{"alert", "units", "type", "component", "context", "notification.to", "notification.exec", "summary", "info"},
				long),
			// a facet's values are read before the cut (api_v2_contexts_alert_transitions.c:264-282)
			alertsV2Facets(map[string]string{"f_status": "UNINITIALIZED=1", "f_class": "unknown=1", "f_type": alertsV2StatesLongType + "=1",
				"f_component": alertsV2StatesLongPart + "=1", "f_role": alertsV2StatesLongTo + "=1",
				"f_node": parentIdentity.MachineGUID + "(" + parentIdentity.Hostname + ")=1", "f_alert": alertsV2StatesLongName + "=1",
				"f_instance": "hst.idle=1", "f_context": alertsV2StatesLongContext + "=1"})})),
		alertsV2ByID("gone", ids["hst_gone"], q, dashGuard([]dashFact{change(base, "hst_gone UNINITIALIZED REMOVED 0"),
			one("REMOVED", "hst_gone", "hst.gone", "hst.gctx")})),
	}
}

// --- health.child `half-off` (health_child_test.go): a host whose health never ran beside one with alerts

// alertsV2HalfOff are `half-off`'s rows: localhost's alert WARNING, the child's host without health.
func alertsV2HalfOff(t *testing.T, h *healthPair) {
	t.Helper()
	alertsV2WaitWindow(t, h, h.n, "")
	fam := alertsV2Family(h.n, alertsV2LogReader(h))
	for _, req := range alertsV2HalfOffRows() {
		compareV2(t, h.p, req, fam)
	}
}

// alertsV2HalfOffRows are alertsV2HalfOff's rows. Their guards read what C answers: C keeps no count of a host whose
// health never ran, so a status word skips no host (database/contexts/api_v2_contexts.c:578-597, :648-654): the child
// is walked, has no alert, and is listed only without a window (:656-658; `raised`, `plain`), not under one
// (:672-673; `raised-window`, `window`); localhost is listed in each, with its alert. The window's host table holds
// both hosts, and only localhost has transitions (`transitions`: the child has no alert log).
func alertsV2HalfOffRows() []v2Req {
	at := func(query string) string { return alertsV2Route + "?options=summary,instances,minify" + query }
	alert := []alertsV2Rows{
		{member: "alerts", keys: []string{"ati", "ni", "nm", "wr", "in", "nd"}, items: []string{"0 [0] hloc_calc 1 1 1"}},
		{member: "alert_instances", keys: []string{"ni", "nm", "ch", "st"}, items: []string{"0 hloc_calc " + healthTwoHostsChart + " WARNING"}},
		{member: "alerts_by_recipient", keys: alertsV2RecipientKeys, items: []string{"root 0 1 0 1 0 2"}},
		{member: "alerts_by_module", keys: alertsV2ModuleKeys, items: []string{"[none] 0 1 0 1 0"}},
	}
	both := alertsV2Guard(append([]alertsV2Rows{alertsV2Nodes(alertsV2Parent, alertsV2Child)}, alert...)...)
	one := alertsV2Guard(append([]alertsV2Rows{alertsV2Nodes(alertsV2Parent)}, alert...)...)
	return []v2Req{
		{name: "raised", target: at("&status=raised"), status: "200", guard: both},
		{name: "raised-window", target: at("&status=raised&after=-600"), status: "200", guard: one},
		{name: "plain", target: at(""), status: "200", guard: both},
		{name: "window", target: at("&after=-600"), status: "200", guard: one},
		{name: "transitions", target: "/api/v2/alert_transitions?after=-600&last=200&options=minify", status: "200",
			guard: dashGuard([]dashFact{alertsV2Guard(alertsV2Items("1 1 1 200 0 0"), alertsV2Rows{member: "transitions",
				keys: alertsV2TwoHostsKeys, items: []string{"hloc_calc " + parentIdentity.Hostname + " " + parentIdentity.MachineGUID +
					" CLEAR WARNING 70 {EXEC_IN_PROGRESS,EXEC_RUN,PROCESSED,SAVED}"}}),
				alertsV2OneOption(1, "WARNING", parentIdentity.MachineGUID, parentIdentity.Hostname, "hloc_calc", healthTwoHostsChart, "hloc.ctx")})},
	}
}
