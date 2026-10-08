// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/json"
	"fmt"
	"net/http"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The alert endpoints of the contexts v2 engine (milestone 10, D224; plan evidence/2026-10-06-plan-m10-commit0.md §3):
// `/api/v2/alerts` and `/api/v3/alerts` read the alerts' live state, `/api/v2/alert_transitions` and
// `/api/v3/alert_transitions` the alert log's transitions in SQLite (database/contexts/api_v2_contexts_alerts.c,
// api_v2_contexts_alert_transitions.c, database/sqlite/sqlite_health.c:1458-1652). They run on the health runner: both
// agents hold the same alerts and transitions, each with ids and clocks of its own, which the families render on each
// side, against that side's own alert log, before the answers are parsed (alertsV2Render).

var (
	// the members that hold a wall-clock second or a span between two events: an alert's last evaluation and last change
	// (`t`, `tr_t`: api_v2_contexts_alerts.c:362, :377), a transition's time, its notification's and the end of its delay
	// (`when`, `delay_up_to_time`: api_v2_contexts_alert_transitions.c:431, :454, :456), the spans before it (`duration`,
	// `raised_duration`: :447-448); and an entry's global id, the microseconds at its creation (`gi`:
	// api_v2_contexts_alerts.c:337, api_v2_contexts_alert_transitions.c:400; health/health_log.c:226)
	alertsV2ClockRe = regexp.MustCompile(`"(gi|t|tr_t|when|delay_up_to_time|duration|raised_duration)":(\s*)(\d+)`)
	// an alert's last transition id and a transition's id: random UUIDs (health/health_log.c:225;
	// api_v2_contexts_alerts.c:360, api_v2_contexts_alert_transitions.c:404)
	alertsV2TidRe = regexp.MustCompile(`"(tr_i|transition_id)":(\s*)"([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})"`)
	// an alert's or a transition's global id, the member each of them has once, before its other clocks
	// (api_v2_contexts_alerts.c:337, api_v2_contexts_alert_transitions.c:400)
	alertsV2ItemRe = regexp.MustCompile(`"gi":\s*\d+`)
)

// alertsV2Entry is what the alert endpoints' times and spans are read against: an entry of the side's own alert log,
// `/api/v1/alarm_log`. Both endpoints print the entry's stored columns (database/sqlite/sqlite_health.c:1063-1070,
// :1158-1185: the log; :1462-1466 and api_v2_contexts_alert_transitions.c:431-456: a transition), and an alert its
// last entry's id and the second of its last change (health/health_log.c:225, :238; api_v2_contexts_alerts.c:360-362).
// The members are the ones the render reads (healthEntry, the health checks' view of an entry, has no end of the
// delay).
type alertsV2Entry struct {
	Tid       string `json:"transition_id"`
	Status    string `json:"status"`
	OldStatus string `json:"old_status"`
	When      int64  `json:"when"`
	Duration  int64  `json:"duration"`
	NonClear  int64  `json:"non_clear_duration"`
	ExecRun   int64  `json:"exec_run"`
	DelayUpTo int64  `json:"delay_up_to_timestamp"`
}

// lastChange is the second an alert whose last entry is e changed status last, as C keeps it for the alert: the
// entry's `when` (a status change's entry and the alert take the pass's one clock, health/health_event_loop.c:738-741,
// :768), but for an alert that never had a status, whose last entry is its link's, from REMOVED to UNINITIALIZED
// (made for a new alert alone, health/rrdcalc.c:457): C takes one clock when it makes the alert (:413) and reads
// another for the link's entry, whose duration is the seconds between the two (:320, :325). An alert that left
// REMOVED by a status change has an entry from REMOVED too, to a status that is never UNINITIALIZED
// (health/health_event_loop.c:518-519, :668-690): its last change is that entry's `when`.
func (e alertsV2Entry) lastChange() int64 {
	if e.OldStatus == "REMOVED" && e.Status == "UNINITIALIZED" {
		return e.When - e.Duration
	}
	return e.When
}

// alertsV2Log is a side's alert log: its entries by transition id.
type alertsV2Log map[string]alertsV2Entry

// alertsV2LogOf reads an `/api/v1/alarm_log` body.
func alertsV2LogOf(body []byte) (alertsV2Log, error) {
	var entries []alertsV2Entry
	if err := json.Unmarshal(body, &entries); err != nil {
		return nil, err
	}
	log := alertsV2Log{}
	for _, e := range entries {
		if e.Tid != "" {
			log[e.Tid] = e
		}
	}
	return log, nil
}

// alertsV2Render renders one side's alerts or transitions body for the comparison, each id, time and span by what it
// is in the side's own alert log (log; n names the ids), and only where it is that. Each alert instance and each
// transition is read from its global id to the next one's (alertsV2ItemRe) against the log's entry its transition id
// names (`tr_i`, `transition_id`):
//   - the transition id reads `t+k`, the entry's unique id from the side's base (healthNorm.tid);
//   - a transition's `when` reads WHEN where it is the entry's `when`, and an alert's last change (`tr_t`) where it
//     is the second the entry says the alert changed last (alertsV2Entry.lastChange);
//   - a transition's `duration` and `raised_duration` read DURATION and NON_CLEAR where they are the entry's
//     `duration` and `non_clear_duration`; its notification's `when` and `delay_up_to_time` read EXEC_RUN and
//     DELAY_UP_TO where they are the entry's `exec_run` and `delay_up_to_timestamp`;
//   - the global id reads G where it is a microsecond of the entry's `when` or of the healthBound seconds after it:
//     C reads the clock for it as it makes the entry (health/health_log.c:226), after the one that gave `when`, with
//     no bound of its own; when the body has two or more, only if some global id is no whole second (one alone cannot
//     tell);
//   - an alert's last evaluation (`t`) reads T where it is a second of the request's flight or of the healthBound
//     seconds before it (an alert that is evaluated is evaluated each second in these cases; one that never was has
//     0).
//
// A 0 stays 0. Anything else is left as the agent wrote it, for the comparison: an id the log does not hold, a time
// that is not its entry's, a global id in seconds. It returns the seconds it replaced in the body's order (a global
// id's whole seconds), for the bound beside the render (healthClocksNear).
func alertsV2Render(n *healthNorm, log alertsV2Log, flight [2]int64, body []byte) ([]byte, []int64) {
	micro, ids := false, alertsV2ItemRe.FindAll(body, -1)
	for _, m := range ids {
		_, digits, _ := strings.Cut(string(m), ":")
		if v, err := strconv.ParseInt(strings.TrimSpace(digits), 10, 64); err == nil && v%1_000_000 != 0 {
			micro = true
		}
	}
	micro = micro || len(ids) < 2
	var clocks []int64
	item := func(span string) string {
		// the item's one transition id (an alert's `tr_i`, a transition's `transition_id`) and its entry
		var entry alertsV2Entry
		known := false
		if g := alertsV2TidRe.FindStringSubmatch(span); g != nil {
			entry, known = log[g[3]]
		}
		whens := 0
		span = alertsV2ClockRe.ReplaceAllStringFunc(span, func(m string) string {
			g := alertsV2ClockRe.FindStringSubmatch(m)
			v, err := strconv.ParseInt(g[3], 10, 64)
			if g[1] == "when" {
				whens++
			}
			if err != nil || v == 0 {
				return m
			}
			mark, second := "", v
			switch {
			case g[1] == "t":
				if v >= flight[0]-healthBound && v <= flight[1] {
					mark = "T"
				}
			case !known:
			case g[1] == "gi":
				if second = v / 1_000_000; micro && second >= entry.When && second <= entry.When+healthBound {
					mark = "G"
				}
			case g[1] == "tr_t":
				if v == entry.lastChange() {
					mark = "WHEN"
				}
			case g[1] == "when" && whens == 1:
				if v == entry.When {
					mark = "WHEN"
				}
			case g[1] == "when":
				if v == entry.ExecRun {
					mark = "EXEC_RUN"
				}
			case g[1] == "duration":
				if v == entry.Duration {
					mark = "DURATION"
				}
			case g[1] == "raised_duration":
				if v == entry.NonClear {
					mark = "NON_CLEAR"
				}
			case g[1] == "delay_up_to_time":
				if v == entry.DelayUpTo {
					mark = "DELAY_UP_TO"
				}
			}
			if mark == "" {
				return m
			}
			clocks = append(clocks, second)
			return `"` + g[1] + `":` + g[2] + `"` + mark + `"`
		})
		if !known {
			return span
		}
		return alertsV2TidRe.ReplaceAllStringFunc(span, func(m string) string {
			g := alertsV2TidRe.FindStringSubmatch(m)
			return `"` + g[1] + `":` + g[2] + `"` + n.tid(g[3]) + `"`
		})
	}
	var out strings.Builder
	last := 0
	for k, loc := range alertsV2ItemRe.FindAllIndex(body, -1) {
		if k == 0 {
			out.Write(body[:loc[0]])
		} else {
			out.WriteString(item(string(body[last:loc[0]])))
		}
		last = loc[0]
	}
	if len(ids) == 0 {
		return body, nil
	}
	out.WriteString(item(string(body[last:])))
	return []byte(out.String()), clocks
}

// alertsV2LogReader reads side i's alert log as the health pair h serves it now.
func alertsV2LogReader(h *healthPair) func(i int) ([]byte, error) {
	return func(i int) ([]byte, error) {
		r := healthGet(h.p.Each()[i].Daemon, "/api/v1/alarm_log")
		if r.Status != http.StatusOK {
			return nil, fmt.Errorf("/api/v1/alarm_log answered %d: %s", r.Status, truncateBytes(r.Body))
		}
		return r.Body, nil
	}
}

// alertsV2Family compares the alert endpoints' answers, `/api/v2|v3/alerts` and `/api/v2|v3/alert_transitions`. With
// the health runner's normalizers (n[i] side i's) and a reader of each side's alert log (log, read again after every
// answer: an entry's notification is stored after the entry): each side's body rendered against its own log
// (alertsV2Render), the seconds the render replaced held within healthBound side to side, and healthCandidateWait for
// the candidate to show the oracle's answer. A side whose log cannot be read has its body left unparsable, with the
// reason. With the zero pair (health off: the dashboard replay, whose agents hold no alert) the v2 envelope's masks
// alone, without a render, a bound or a wait.
func alertsV2Family(n [2]*healthNorm, log func(i int) ([]byte, error)) v2Family {
	if n[0] == nil || n[1] == nil {
		return v2Family{masks: infoV2Volatile}
	}
	var clocks [2][]int64
	return v2Family{
		masks:  infoV2Volatile,
		settle: healthCandidateWait,
		render: func(i int, flight [2]int64, body []byte) []byte {
			raw, err := log(i)
			var entries alertsV2Log
			if err == nil {
				entries, err = alertsV2LogOf(raw)
			}
			if err != nil {
				return append([]byte("the side's alert log: "+err.Error()+"\n"), body...)
			}
			n[i].observe(string(raw))
			out, c := alertsV2Render(n[i], entries, flight, body)
			clocks[i] = c
			return out
		},
		check: func(t *testing.T, name string, _, _ Value) {
			t.Helper()
			if err := healthClocksNear(clocks[0], clocks[1]); err != nil {
				t.Errorf("%s: %v", name, err)
				return
			}
			apart := int64(0)
			for k := range clocks[0] {
				apart = max(apart, clocks[0][k]-clocks[1][k], clocks[1][k]-clocks[0][k])
			}
			t.Logf("%s: the two sides' %d times and spans are at most %d s apart (the bound is %d)", name,
				len(clocks[0]), apart, healthBound)
		},
	}
}

// alertsV2Rows is what a guard reads of one member of an answer: for an array, the members `keys` of each item, one
// line per item (the values joined by spaces; a key with dots names a nested member), compared as a sorted set when
// `sorted` (the order is the comparison's to judge); for an object, one line of its members `keys`. A nil `items`
// wants the answer without the member.
type alertsV2Rows struct {
	member string
	keys   []string
	items  []string
	sorted bool
}

// alertsV2Guard is a guard on a v2 answer: each of rows as it says.
func alertsV2Guard(rows ...alertsV2Rows) func(Value) error {
	return func(v Value) error {
		for _, r := range rows {
			m, err := dashMember(v, r.member)
			if r.items == nil {
				if err == nil {
					return fmt.Errorf("the answer has %s", r.member)
				}
				continue
			}
			if err != nil {
				return err
			}
			items := m.Items
			if m.Kind == KindObject {
				items = []Value{m}
			}
			var got []string
			for _, item := range items {
				var fields []string
				for _, k := range r.keys {
					f, err := dashMember(item, strings.Split(k, ".")...)
					if err != nil {
						return fmt.Errorf("%s: %v", r.member, err)
					}
					if f.Kind == KindString {
						fields = append(fields, f.Text)
					} else {
						fields = append(fields, f.String())
					}
				}
				got = append(got, strings.Join(fields, " "))
			}
			want := r.items
			if r.sorted {
				slices.Sort(got)
				want = slices.Sorted(slices.Values(r.items))
			}
			if !slices.Equal(got, want) {
				return fmt.Errorf("%s (%s) is %q, want %q", r.member, strings.Join(r.keys, " "), got, want)
			}
		}
		return nil
	}
}

// alertsV2AccessCase runs a case's access rows (each configuration's pair has no health, D224 F1) once its health
// pair stopped: the routes' ACL is the dashboard's (HTTP_ACL_ALERTS, libnetdata/user-auth/http-access.h:103;
// web/api/web_api.c:82-96: 451 from a client the list leaves out) and their access anonymous data (412 for an
// anonymous client under bearer protection, web/server/web_client.c:57-70): accessRoutes.
func alertsV2AccessCase(routes ...string) func(t *testing.T, h *healthPair) {
	return func(t *testing.T, _ *healthPair) {
		t.Run("access", func(t *testing.T) { accessRows(t, []accessConf{accessACL, accessBearer}, accessRoutes(routes...)) })
	}
}

// TestAlertsV2 (checks `api.v2-alerts` = `TestAlertsV2/alerts` and `api.v2-alert-transitions` =
// `TestAlertsV2/transitions`, milestone 10 commit 0, D224; red on the Rust agent until commits 4 and 5): the alert
// endpoints of the contexts v2 engine, on the health runner. Each case compares a v1 answer of the same state first
// (the green anchor), reads each side's alert log for its ids' aliases, then asks the v2 endpoints (compareV2: the
// oracle's status and guard, then the candidate's answer within healthCandidateWait), and once both agents stopped,
// its routes' access rows (`access`). Cases:
//   - `alerts`: `health.api` `endpoints`' three alerts, one WARNING, and a fourth on a chart never collected
//     (alertsV2Conf): `/api/v3/alerts` with `status=raised`, with the summary alone, with `alert=` of two names, and
//     `/api/v2/alerts` pretty;
//   - `transitions`: `health.transitions`' three alerts through CLEAR, WARNING, CRITICAL and CLEAR:
//     `/api/v2/alert_transitions` over the last ten minutes, its newest four after the second switch (`anchor_gi`), and
//     `/api/v3/alert_transitions` of one transition by its id, each side's own (v2Req.targets): a change to WARNING,
//     which the window lists too, and a first status, which it does not.
//
// Neither endpoint runs a data query: asking them does not pause HEALTH (stream-control.c:99-103).
func TestAlertsV2(t *testing.T) {
	runHealthCases(t, map[string]healthCase{
		"alerts": {
			conf:  alertsV2Conf,
			sc:    alertsV2Scenario(),
			play:  alertsV2PlayAlerts,
			after: alertsV2AccessCase("/api/v2/alerts", "/api/v3/alerts"),
		},
		"transitions": {
			conf:  healthSigConf,
			grid:  healthSigGrid,
			sc:    healthValues("hsig.values", "hsig.ctx", []string{"a"}, healthSigPhases...),
			play:  alertsV2PlayTransitions,
			after: alertsV2AccessCase("/api/v2/alert_transitions", "/api/v3/alert_transitions"),
		},
	})
}

// alertsV2Module is the module of the `alerts` case's collected chart (plugin.Values.Module). C groups alerts by their
// chart's `_collect_module` label (api_v2_contexts_alerts.c:141-151), which holds the module sanitized as a label
// value (rrdset-index-id.c:23-28; rrdlabels.c:351), where a comma becomes a period (sanitizers-labels.c:69).
const alertsV2Module = "parity,mod"

// alertsV2PlainEmit defines the `alerts` case's second chart, of the same context, without a module (C labels it
// "[none]": sanitizers-labels.c:152) and never collected (healthScenario's lines).
const alertsV2PlainEmit = "CHART hsig.plain '' 'title' 'units' 'family' 'hsig.ctx' line 1000 1 '' '' ''\n" +
	"DIMENSION b '' absolute 1 1\n"

// alertsV2Conf are `health.api` `endpoints`' three alerts (healthAPIConf) and hm_plain on the chart never collected,
// with a class, a type and a component (the groupings by them, api_v2_contexts_alerts.c:106-132, :385-392).
const alertsV2Conf = healthAPIConf + `
 alarm: hm_plain
    on: hsig.plain
  calc: $b
 every: 1s
  warn: $this > 1000
 units: things
 class: Workload
  type: Parity
component: Fixture
  info: b above 1000
`

// alertsV2Scenario is the `alerts` case's plugin: hsig.values of `endpoints` with alertsV2Module, created after
// hsig.plain (alertsV2PlainEmit) in one write, so both agents create the two charts in one order.
func alertsV2Scenario() *plugin.Scenario {
	sc := healthScenario(alertsV2PlainEmit, "hsig.values", "hsig.ctx", []string{"a"}, map[string]int64{"a": 10},
		map[string]int64{"a": 70})
	healthChart(sc).Module = alertsV2Module
	return sc
}

// alertsV2PlayAlerts plays `alerts`: the endpoint check's state (health.api `endpoints`: a = 70, ha_low WARNING, ha_mid
// and ha_high CLEAR), then the v2 rows.
func alertsV2PlayAlerts(t *testing.T, h *healthPair) {
	h.create(t)
	h.waitOracle(t, "the chart's alerts", func() (string, error) {
		v := h.get(0, "/api/v1/alarms?all")
		return v, healthAll("CLEAR", "ha_low", "ha_mid", "ha_high")(v)
	})
	h.release(t, "p1", 1, healthCalcHold)
	// the green anchor, as health.api compares it
	h.compareNow(t, "/api/v1/alarms?all", func(i int) string { return h.get(i, "/api/v1/alarms?all") },
		healthWant(map[string]string{"ha_low": "WARNING", "ha_mid": "CLEAR", "ha_high": "CLEAR"}))
	// the ids' aliases: each side's alert log, once it holds ha_low's change (an alert's `tr_i` is its last entry's id)
	h.compareNow(t, "the alert log's transitions", func(i int) string { return h.transitions(i, "") },
		alertsV2LogHolds("ha_low: CLEAR->WARNING 70 things"))
	fam := alertsV2Family(h.n, alertsV2LogReader(h))
	for _, req := range alertsV2AlertRows {
		compareV2(t, h.p, req, fam)
	}
}

// alertsV2AlertRows are the `alerts` case's v2 rows. Their guards read what C answers for alertsV2Conf at a = 70:
//   - the summary (`alerts`, api_v2_contexts_alerts.c:627-683) has one item per alert name the filters keep, with its
//     instances by status (:183-208), in the order the alerts were met: hsig.plain's hm_plain first, its chart created
//     first (the context's instances in creation order); hm_plain is UNINITIALIZED (its chart is never collected), in
//     no status column (:197-199);
//   - the groupings count the running alerts the filters keep (:106-151, :228-248): by the rule's type, component,
//     classification and recipient, which also count every rule (`available`: :385-392, :685, so a grouping's entry
//     is there with `running` 0 when no alert of it is kept), and by the chart's `_collect_module` label, never
//     `available`: hsig.plain's "[none]", hsig.values' alertsV2Module sanitized;
//   - `status=raised` keeps an alert whose status is WARNING or above (:85-87): ha_low alone;
//   - `alert=` matches the alerts' names (:61-62): ha_mid and ha_high;
//   - `alert_instances` (:321-383) is there only with `instances` or `values` (:693-695); `v` is the last value (:376),
//     `tr_v` the value of the last change (:361).
var alertsV2AlertRows = func() []v2Req {
	module := []string{"name", "cr", "wr", "cl", "running"}
	group := []string{"name", "running", "available"}
	both := []string{"[none] 0 0 0 1", "parity.mod 0 1 2 3"}
	groupings := func(running string) []alertsV2Rows {
		return []alertsV2Rows{
			{member: "alerts_by_type", keys: group, items: []string{"Parity " + running + " 1"}},
			{member: "alerts_by_component", keys: group, items: []string{"Fixture " + running + " 1"}},
			{member: "alerts_by_classification", keys: group, items: []string{"Workload " + running + " 1"}},
		}
	}
	recipient := []string{"name", "cr", "wr", "cl", "running", "available"}
	return []v2Req{
		{name: "raised", target: "/api/v3/alerts?options=summary,values,instances,minify&status=raised", status: "200",
			guard: alertsV2Guard(append([]alertsV2Rows{
				{member: "alerts", keys: []string{"nm", "cr", "wr", "cl", "er", "in", "nd", "cfg"}, items: []string{"ha_low 0 1 0 0 1 1 1"}},
				{member: "alert_instances", keys: []string{"nm", "st", "v", "tr_v"}, items: []string{"ha_low WARNING 70 70"}},
				{member: "alerts_by_recipient", keys: recipient, items: []string{"root 0 1 0 1 4"}},
				{member: "alerts_by_module", keys: module, items: []string{"parity.mod 0 1 0 1"}},
			}, groupings("0")...)...)},
		{name: "configs", target: "/api/v3/alerts?options=minify,summary", status: "200",
			guard: alertsV2Guard(append([]alertsV2Rows{
				{member: "alerts", keys: []string{"nm", "cr", "wr", "cl"},
					items: []string{"hm_plain 0 0 0", "ha_low 0 1 0", "ha_mid 0 0 1", "ha_high 0 0 1"}},
				{member: "alert_instances"},
				{member: "alerts_by_recipient", keys: recipient, items: []string{"root 0 1 2 4 4"}},
				{member: "alerts_by_module", keys: module, items: both},
			}, groupings("1")...)...)},
		{name: "by-name", target: "/api/v3/alerts?options=summary,values,instances,minify&alert=ha_mid%7Cha_high", status: "200",
			guard: alertsV2Guard(
				alertsV2Rows{member: "alerts", keys: []string{"nm", "wr", "cl"}, items: []string{"ha_mid 0 1", "ha_high 0 1"}},
				alertsV2Rows{member: "alert_instances", keys: []string{"nm", "st", "v", "tr_v"}, items: []string{"ha_mid CLEAR 70 10", "ha_high CLEAR 70 10"}},
				alertsV2Rows{member: "alerts_by_module", keys: module, items: []string{"parity.mod 0 0 2 2"}})},
		// pretty, with the walk's `"api":2` (database/contexts/api_v2_contexts.c:1379-1380; the layout is compared too)
		{name: "v2", target: "/api/v2/alerts?options=summary,instances,values", status: "200",
			guard: dashGuard([]dashFact{dashIs("2", "api"), alertsV2Guard(
				alertsV2Rows{member: "alerts", keys: []string{"nm", "wr", "cl"},
					items: []string{"hm_plain 0 0", "ha_low 1 0", "ha_mid 0 1", "ha_high 0 1"}},
				alertsV2Rows{member: "alert_instances", keys: []string{"nm", "st", "v"},
					items: []string{"hm_plain UNINITIALIZED null", "ha_low WARNING 70", "ha_mid CLEAR 70", "ha_high CLEAR 70"}},
				alertsV2Rows{member: "alerts_by_module", keys: module, items: both})})},
	}
}()

// alertsV2OneClear is the `one-clear` row's guard: the one transition is hs_calc's first status, from UNINITIALIZED
// to CLEAR at the first value, its notification never run (`when` 0), and it is the one row evaluated, matched and
// returned.
var alertsV2OneClear = alertsV2Guard(
	alertsV2Rows{member: "transitions", keys: append(slices.Clone(alertsV2TransitionKeys), "notification.when"),
		items: []string{"hs_calc UNINITIALIZED CLEAR 10 0"}},
	alertsV2Rows{member: "items", keys: []string{"evaluated", "matched", "returned", "max_to_return", "before", "after"},
		items: []string{"1 1 1 1 0 0"}})

// alertsV2LogHolds is a guard on a transitions view (healthPair.transitions): it holds each of the lines.
func alertsV2LogHolds(lines ...string) func(string) error {
	return func(view string) error {
		got := strings.Split(view, "\n")
		for _, l := range lines {
			if !slices.Contains(got, l) {
				return fmt.Errorf("no %q", l)
			}
		}
		return nil
	}
}

// the transitions health.transitions' alerts make with a status above CLEAR on a side (sqlite_health.c:1474), as
// alertsV2Guard reads them: hs_calc and hs_max step with the value, hs_avg with its aligned window's average (85 is
// WARNING, 44 CLEAR: healthGridSecond)
var (
	alertsV2TransitionKeys = []string{"alert", "old.status", "new.status", "new.value"}
	alertsV2Newest         = []string{"hs_avg CRITICAL CLEAR 44", "hs_avg WARNING CRITICAL 95", "hs_calc CRITICAL CLEAR 10",
		"hs_max CRITICAL CLEAR 10"}
	alertsV2Transitions = append([]string{"hs_avg CLEAR WARNING 70", "hs_calc CLEAR WARNING 70", "hs_calc WARNING CRITICAL 95",
		"hs_max CLEAR WARNING 70", "hs_max WARNING CRITICAL 95"}, alertsV2Newest...)
)

// alertsV2PlayTransitions plays `transitions`: health.transitions' phases (each switched at the third second of
// hs_avg's window, and compared through /api/v1/alarms?all), the alert log's transitions (the green anchor), then the
// v2 rows. Their guards read what C answers:
//   - the window's rows are the transitions with a status above CLEAR on a side (sqlite_health.c:1474: three per
//     alert, the links' entries left out), newest first (:1558; api_v2_contexts_alert_transitions.c:187-255 keeps
//     that order);
//   - `items` counts them (api_v2_contexts_alert_transitions.c:495-515): `evaluated` the rows of the window, `matched`
//     those the facets keep, `before` those at or before `anchor_gi` (:190-194), `after` those past `last` (:215-219,
//     :241-254), `returned` the rest;
//   - with `transition=` the one row of that id, whatever its status (sqlite_health.c:1476-1479, :1502-1512: the
//     direct statement has no status clause, so hs_calc's first status, UNINITIALIZED to CLEAR, answers too, where the
//     window leaves it out), and `last` 1 when the request has none (web/api/v2/api_v2_contexts.c:70-71).
//
// The phases are health.transitions' (healthPlayPhases), played here to keep the second of the second switch.
func alertsV2PlayTransitions(t *testing.T, h *healthPair) {
	names := []string{"hs_avg", "hs_calc", "hs_max"}
	var switched int64
	h.create(t)
	for k := range healthSigPhases {
		if k > 0 {
			if second := h.release(t, fmt.Sprintf("p%d", k), k, healthSigHold); k == 2 {
				switched = second
			}
		}
		h.compareNow(t, fmt.Sprintf("phase %d: /api/v1/alarms?all", k), func(i int) string { return h.get(i, "/api/v1/alarms?all") },
			healthAll(healthSigStatus[k], names...))
	}
	// the green anchor, as health.transitions compares it; it also names each side's transition ids
	h.compareNow(t, "the alert log's transitions", func(i int) string { return h.transitions(i, "") },
		alertsV2LogHolds("hs_calc: CLEAR->WARNING 70 things", "hs_avg: CRITICAL->CLEAR 44 things"))
	// one transition by its id, each side's own: hs_calc's change to WARNING and its first status
	var raised, first [2]string
	var newest int64
	for i, side := range h.p.Each() {
		entries, err := h.entriesAs(h.n[i], i, "/api/v1/alarm_log")
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		for _, e := range entries {
			if e.Name == "hs_calc" && e.OldStatus == "CLEAR" && e.Status == "WARNING" {
				raised[i] = e.Tid
			}
			if e.Name == "hs_calc" && e.OldStatus == "UNINITIALIZED" && e.Status == "CLEAR" {
				first[i] = e.Tid
			}
			if i == 0 {
				newest = max(newest, e.When)
			}
		}
		for what, id := range map[string]string{"from CLEAR to WARNING": raised[i], "from UNINITIALIZED to CLEAR": first[i]} {
			if id != "" {
				continue
			}
			if side.Role == Oracle {
				t.Fatalf("oracle: its alert log has no change of hs_calc %s", what)
			}
			// the row is still asked, without an id: it shows what the candidate answers then
			t.Errorf("candidate: its alert log has no change of hs_calc %s", what)
		}
	}
	// a relative window ends a second before the request's (libnetdata/libnetdata.c:550-551), at that second's first
	// microsecond (sqlite_health.c:1474, :1566-1567): the oracle's newest change (made in its second `when`, its global
	// id later in that second) is in the window from `when`+2 on; one more second for a side a second behind (seen C
	// against C, 2026-10-06 20:23Z: asked at `when`+1, both sides left the two newest out)
	time.Sleep(time.Until(time.Unix(newest+3, 0)))
	items := []string{"evaluated", "matched", "returned", "max_to_return", "before", "after"}
	one := "/api/v3/alert_transitions?options=minify&transition="
	fam := alertsV2Family(h.n, alertsV2LogReader(h))
	for _, req := range []v2Req{
		{name: "window", target: "/api/v2/alert_transitions?after=-600&last=200&options=minify", status: "200",
			guard: alertsV2Guard(
				alertsV2Rows{member: "transitions", keys: alertsV2TransitionKeys, items: alertsV2Transitions, sorted: true},
				alertsV2Rows{member: "items", keys: items, items: []string{"9 9 9 200 0 0"}})},
		// the newest four after the second switch: phase 3's three changes and hs_avg's change to CRITICAL, which its
		// window shows after phase 2 began (S2 = the switch's second, one value for both sides)
		{name: "anchor", target: fmt.Sprintf("/api/v2/alert_transitions?after=-600&last=4&anchor_gi=%d&options=minify", switched*1_000_000),
			status: "200",
			guard: alertsV2Guard(
				alertsV2Rows{member: "transitions", keys: alertsV2TransitionKeys, items: alertsV2Newest, sorted: true},
				alertsV2Rows{member: "items", keys: items, items: []string{"9 9 4 4 3 2"}})},
		{name: "one", target: one + "<hs_calc's change to WARNING>", targets: [2]string{one + raised[0], one + raised[1]},
			status: "200",
			guard: alertsV2Guard(
				alertsV2Rows{member: "transitions", keys: alertsV2TransitionKeys, items: []string{"hs_calc CLEAR WARNING 70"}},
				alertsV2Rows{member: "items", keys: items, items: []string{"1 1 1 1 0 0"}})},
		// a status the window leaves out (neither side of it is above CLEAR): asked by its id it answers
		{name: "one-clear", target: one + "<hs_calc's first status>", targets: [2]string{one + first[0], one + first[1]},
			status: "200", guard: alertsV2OneClear},
	} {
		compareV2(t, h.p, req, fam)
	}
}
