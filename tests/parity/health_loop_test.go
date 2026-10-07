// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"os"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// healthAsk is one answer a loop case compares: a v1 alert endpoint's whole body (healthNorm.json), with the oracle's
// guard.
type healthAsk struct {
	path  string
	guard func(string) error
}

// asks compares each answer in turn, named `<at><path>`.
func (h *healthPair) asks(t *testing.T, at string, asks ...healthAsk) {
	t.Helper()
	for _, a := range asks {
		h.compareNow(t, at+a.path, func(i int) string { return h.get(i, a.path) }, a.guard)
	}
}

// healthBoth is a guard that holds when each of the guards does.
func healthBoth(guards ...func(string) error) func(string) error {
	return func(view string) error {
		for _, g := range guards {
			if err := g(view); err != nil {
				return err
			}
		}
		return nil
	}
}

// healthGone is a guard on /api/v1/chart of a chart C does not serve (an obsolete one): 404 and its text.
func healthGone(chart string) func(string) error {
	return func(view string) error {
		if want := "HTTP 404, text/plain; charset=utf-8\nChart is not found: " + chart; view != want {
			return fmt.Errorf("answered %q, want %q", view, want)
		}
		return nil
	}
}

// healthCount is a guard on an /api/v1/alarm_count answer: 200 and `[n]`.
func healthCount(n int) func(string) error { return healthIs(fmt.Sprintf("[%d]\n", n)) }

// healthLatest is a guard on an /api/v1/alarms answer: the host gave k unique ids so far.
func healthLatest(k int) func(string) error {
	return healthHolds(fmt.Sprintf("\"latest_alarm_log_unique_id\": u+%d,\n", k))
}

// an alert's key in /api/v1/alarms_values, then its id, value and last update, then its status (health_json.c:16-38)
var healthValueStatusRe = regexp.MustCompile(`"([^"\n]+)": \{\n\t\t\t"id": [^\n]*\n\t\t\t"value":[^\n]*\n\t\t\t"last_updated":[^\n]*\n\t\t\t"status": "([A-Z]+)"`)

// healthValuesWant is a guard on a view of /api/v1/alarms_values: each alert, by its `chart.name` key, has its status,
// and no other alert shows.
func healthValuesWant(want map[string]string) func(string) error {
	return func(view string) error {
		if err := healthHolds()(view); err != nil {
			return err
		}
		got := map[string]string{}
		for _, m := range healthValueStatusRe.FindAllStringSubmatch(healthBody(view), -1) {
			got[m[1]] = m[2]
		}
		if !maps.Equal(got, want) {
			return fmt.Errorf("the alerts are %v, want %v", got, want)
		}
		return nil
	}
}

// healthLogWant is a guard on a side's health.log lines: n records, and for each of `records` a line that holds all
// its parts.
func healthLogWant(n int, records ...[]string) func([]string) error {
	return func(lines []string) error {
		if len(lines) != n {
			return fmt.Errorf("%d records, want %d", len(lines), n)
		}
		for _, parts := range records {
			if !slices.ContainsFunc(lines, func(l string) bool {
				return !slices.ContainsFunc(parts, func(p string) bool { return !strings.Contains(l, p) })
			}) {
				return fmt.Errorf("no record with %q", parts)
			}
		}
		return nil
	}
}

// healthBothLines is a guard on lines that holds when each of the guards does.
func healthBothLines(guards ...func([]string) error) func([]string) error {
	return func(lines []string) error {
		for _, g := range guards {
			if err := g(lines); err != nil {
				return err
			}
		}
		return nil
	}
}

// healthLoopLog compares health.log after the stop, in file order: every record with its fields and its message, the
// transition ids as ranks (healthNorm.healthLogRanked: the check reads no alert log).
func healthLoopLog(guard func([]string) error) func(t *testing.T, h *healthPair) {
	return func(t *testing.T, h *healthPair) {
		t.Helper()
		h.compareLines(t, "health.log", func(i int) []string { return h.n[i].healthLogRanked(t, h.p.Each()[i].Daemon) }, guard)
	}
}

// healthFiltered is a guard on the `flags` case's data view (dataAlerts) under a filter, at a = 70: the host's alert
// versions (42 link entries, `entries` so far), the summary's alerts from hf_before on, and hf.values kept (its 14
// alerts counted in itself, its context and its node, and its tree's alerts from hf_before on) or left out (listed,
// nothing counted, no tree).
func healthFiltered(entries int, kept bool) func(string) error {
	counts := `{"cl":7,"wr":4,"cr":1,"ot":2}`
	if !kept {
		counts = "none"
	}
	parts := []string{fmt.Sprintf("versions.alerts_hard_hash: %d\nversions.alerts_soft_hash: %d\n", 14*3, entries),
		`summary.alerts: [{"nm":"hf_before","wr":1},{"nm":"hf_base","wr":1},`,
		fmt.Sprintf("summary.nodes[0] mg=%q: %s\n", parentIdentity.MachineGUID, counts),
		`summary.contexts[0] id="hf.ctx": ` + counts + "\n", `summary.instances[0] id="hf.values": ` + counts}
	if kept {
		parts = append(parts, `detailed hf.ctx hf.values: {"hf_before":{"st":"WARNING","vl":70,`)
	}
	return func(view string) error {
		if err := healthHolds(parts...)(view); err != nil {
			return err
		}
		if !kept && strings.Contains(view, "\ndetailed ") {
			return fmt.Errorf("the instance left out has a tree")
		}
		return nil
	}
}

// healthChart is the collected chart of a scenario (healthScenario), for what the constructor does not take: a family,
// labels, a phase's own lines.
func healthChart(sc *plugin.Scenario) *plugin.Values {
	steps := sc.Starts[0].Steps
	return steps[len(steps)-1].Values
}

// healthFlagsConf are the `flags` case's rules on hf.values (`a`): what an expression that fails, a lookup that finds
// nothing and a rule without a threshold leave in an alert, the units' texts, and alerts that name alerts.
//   - hf_before, hf_base, hf_after: hf_base reads `a`; the two others read hf_base by its name, one defined before it
//     and one after. A pass computes every value before it judges any (health_event_loop.c:441-633, :638-791), in the
//     order the alerts were linked, and a name gives the other alert's value as it is at that moment
//     (health_variable.c:227-232): hf_before sees hf_base's value of the pass before, hf_after this pass's.
//   - hf_ping and hf_pong name each other, hf_self itself: no value ever reaches them.
//   - hf_unknown's calc names nothing that exists; hf_nope's lookup asks for a dimension the chart does not have. Like
//     the three above they have no value, and are CLEAR: `$this > 50` of no value is 0 (isgreater,
//     libnetdata/eval/eval-evaluate.c:118-122), which is a status, CLEAR (health_event_loop.c:121-125).
//   - An expression fails only when its result is no number (eval-evaluate.c:307-321: an unknown variable alone is
//     forgiven): hf_warn_unknown's `$hf_nothing > 1` is 0, CLEAR; hf_warn_fails' `$hf_nothing + 1` fails, and with no
//     other threshold the alert is UNDEFINED, as hf_calc_only, which has none. hf_crit_only has no warning;
//     hf_crit_fails a warning that is CLEAR and a critical that fails: CLEAR. hf_no_clear has the option
//     `no-clear-notification` and a lookup with two options.
//   - units: `%` (no space before it), `seconds` (a duration's text); a rule without units shows its chart's.
//
// The two rules with a lookup come last, look back 5 s, and hf_no_clear is WARNING at both values. A lookup's alert
// first runs when its window has data (health_event_loop.c:173-187): with 5 s that is later than the pass that links
// the alerts and runs the calc alerts (one or two seconds after the chart's first second), so the lookups' first
// entries follow the others' and have a duration on both sides. And it sees a new value in the pass a calc alert
// does or the next: the unique ids of the change to 70 (hf_base, hf_after, hf_crit_only, then hf_before a pass later)
// do not depend on one.
const healthFlagsConf = `# the loop check's rules on hf.values
 alarm: hf_before
    on: hf.values
  calc: $hf_base
 every: 1s
  warn: $this > 50
 units: things

 alarm: hf_base
    on: hf.values
  calc: $a
 every: 1s
  warn: $this > 50
 units: things

 alarm: hf_after
    on: hf.values
  calc: $hf_base
 every: 1s
  warn: $this > 50
 units: things

 alarm: hf_ping
    on: hf.values
  calc: $hf_pong
 every: 1s
  warn: $this > 50

 alarm: hf_pong
    on: hf.values
  calc: $hf_ping
 every: 1s
  warn: $this > 50

 alarm: hf_self
    on: hf.values
  calc: $hf_self + 1
 every: 1s
  warn: $this > 50

 alarm: hf_calc_only
    on: hf.values
  calc: $a * 2
 every: 1s

 alarm: hf_unknown
    on: hf.values
  calc: $hf_nothing + 1
 every: 1s
  warn: $this > 50
 units: things

 alarm: hf_warn_unknown
    on: hf.values
  calc: $a
 every: 1s
  warn: $hf_nothing > 1
 units: things

 alarm: hf_warn_fails
    on: hf.values
  calc: $a
 every: 1s
  warn: $hf_nothing + 1
 units: things

 alarm: hf_crit_only
    on: hf.values
  calc: $a
 every: 1s
  crit: $this > 50
 units: %

 alarm: hf_crit_fails
    on: hf.values
  calc: $a
 every: 1s
  warn: $this > 500
  crit: $hf_nothing + 1
 units: seconds

 alarm: hf_nope
    on: hf.values
lookup: average -5s unaligned of nope
 every: 1s
  warn: $this > 50
 units: things

 alarm: hf_no_clear
    on: hf.values
lookup: max -5s unaligned absolute of a
 every: 1s
  warn: $this > 5
 units: things
options: no-clear-notification
`

// healthDelayConf is the `delay` case's alert: a notification delay of 6 s on the way up and 8 s on the way down,
// doubled at each change of status inside the last delay, 20 s at most (health_event_loop.c:714-736).
const healthDelayConf = `# the loop check's alert on hd.values
 alarm: hd_calc
    on: hd.values
  calc: $a
 every: 1s
  warn: $this > 50
 delay: up 6s down 8s multiplier 2 max 20s
 units: things
  info: the last value of a, delayed
`

// healthRepeatConf is the `repeat` case's alert: while WARNING or CRITICAL it is notified again every 4 s
// (health_event_loop.c:798-874).
const healthRepeatConf = `# the loop check's alert on hr.values
 alarm: hr_calc
    on: hr.values
  calc: $a
 every: 1s
  warn: $this > 50
  crit: $this > 90
repeat: warning 4s critical 4s
 units: things
  info: the last value of a, repeated
`

// The `texts` case: `${family}` and `${label:NAME}` in `summary` and `info` (health/rrdcalc.c:183-260). ht.a is
// collected, with a family and four labels; ht.b is defined before it and never collected, without a family (C gives
// it its type, `ht`: database/rrdset-index-id.c:78) and with one label. C keeps no `$`, `{` or `}` in a label's value
// (`a${family}b` is stored as `a__family_b`) and no `"` in a rule's text (each becomes a space,
// health_config.c:470-475): a value cannot bring a token of its own, and a quote cannot reach the hand-built body. A
// backslash can: the body prints the text as it is (health_json.c:53-70).
var (
	// a token of 99 bytes is replaced, one of 100 is not (rrdcalc.c:189-203: the scan copies 99 bytes at most)
	healthTextsL99  = strings.Repeat("n", 90)
	healthTextsL100 = strings.Repeat("m", 91)
	healthTextsEmit = `CHART ht.b '' 'title' 'units' '' 'ht.ctx' line 1000 1 '' '' ''
DIMENSION a '' absolute 1 1
CLABEL kind bee 1
CLABEL_COMMIT
`
	healthTextsLabels = []string{"kind web one", healthTextsL99 + " v99", healthTextsL100 + " v100", "tricky a${family}b"}
	// the chart's definition again with two labels: `kind` changed, `role` new; the others are gone at the commit.
	// HEALTH's next pass unlinks the chart's alerts and links them again: three entries each, and the texts anew
	healthTextsRelabel = `CHART ht.a '' 'title' 'units' 'fam one' 'ht.ctx' line 1000 1 '' '' ''
CLABEL 'kind' 'web two' 1
CLABEL 'role' 'db' 1
CLABEL_COMMIT
`
	healthTextsConf = `# the loop check's rules on ht.ctx
template: ht_tmpl
      on: ht.ctx
    calc: $a
   every: 1s
    warn: $this > 500
 summary: on ${family}
    info: family ${family}, kind ${label:kind}, role ${label:role}, none ${label:nope}.

   alarm: ht_stop
      on: ht.a
    calc: $a
   every: 1s
    warn: $this > 500
 summary: $x ${family}
    info: ${family} costs $5 on ${family}

   alarm: ht_long
      on: ht.a
    calc: $a
   every: 1s
    warn: $this > 500
 summary: ${label:` + healthTextsL99 + `}
    info: ${label:` + healthTextsL100 + `} then ${family}

   alarm: ht_value
      on: ht.a
    calc: $a
   every: 1s
    warn: $this > 500
 summary: ${label:tricky}/${family}
    info: say "hi" to <b> & back\slash ${family}${family}

   alarm: ht_plain
      on: ht.a
    calc: $a
   every: 1s
    warn: $this > 500
`
)

// The `obsolete` case's chart that is defined obsolete and never collected, and its rules: ho_gone and ho_rep on it
// (ho_rep repeats: C never removes a repeating alert, health_event_loop.c:491), ho_live on the collected chart.
const (
	healthObsoleteEmit = `CHART ho.b '' 'title' 'units' 'family' 'ho.ctx' line 1000 1 'obsolete' '' ''
DIMENSION a '' absolute 1 1
`
	healthObsoleteConf = `# the loop check's rules on ho.a and ho.b
 alarm: ho_gone
    on: ho.b
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: on a chart that is obsolete

 alarm: ho_rep
    on: ho.b
  calc: $a
 every: 1s
  warn: $this > 50
repeat: warning 10s critical 10s
 units: things
  info: on a chart that is obsolete, repeating

 alarm: ho_live
    on: ho.a
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: on a chart that is collected
`
)

// The `obsolete-long` case: ho.old is defined before the collected chart, collected at two seconds (the blocks are
// two phases' own lines, each naming its second), then defined again as obsolete; ho.a goes on.
const (
	healthObsoleteLongEmit = `CHART ho.old '' 'title' 'units' 'family' 'ho.octx' line 1000 1 '' '' ''
DIMENSION a '' absolute 1 1
`
	healthObsoleteLongBlock = "BEGIN ho.old\nSET a = 10\nEND {{sec}} 0\n"
	healthObsoleteLongGone  = "CHART ho.old '' 'title' 'units' 'family' 'ho.octx' line 1000 1 'obsolete' '' ''\n"
	healthObsoleteLongConf  = `# the loop check's rules on ho.a and ho.old
 alarm: ho_old
    on: ho.old
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: on a chart that turns obsolete

 alarm: ho_old_rep
    on: ho.old
  calc: $a
 every: 1s
  warn: $this > 50
repeat: warning 10s critical 10s
 units: things
  info: on a chart that turns obsolete, repeating

 alarm: ho_live
    on: ho.a
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: on a chart that is collected
`
)

// The `count-point` case (D219 K4, D224 F8): hp.side (dimensions a and b) is defined before the collected chart hp.a
// (a), both of context hp.ctx; hp.side is collected at two seconds (each phase's block names its own). The template
// hp_tmpl is on both, hp_lone and hp_never (a lookup over a day: UNINITIALIZED) on hp.a alone.
const (
	healthPointEmit = `CHART hp.side '' 'title' 'units' 'family' 'hp.ctx' line 1000 1 '' '' ''
DIMENSION a '' absolute 1 1
DIMENSION b '' absolute 1 1
`
	healthPointBlock = "BEGIN hp.side\nSET a = 10\nSET b = 1\nEND {{sec}} 0\n"
	healthPointConf  = `# the loop check's rules on hp.ctx
template: hp_tmpl
      on: hp.ctx
    calc: $a
   every: 1s
    warn: $this > 50
   units: things

   alarm: hp_lone
      on: hp.a
    calc: $a
   every: 1s
    crit: $this > 50
   units: things

   alarm: hp_never
      on: hp.a
  lookup: average -1d of a
   every: 1s
    warn: $this > 50
   units: things
`
)

// TestHealthLoop (check `health.loop`, M9 commit 4, D198 F1): what the evaluation loop alone produces, without the
// alert log (`/api/v1/alarm_log` is SQLite's in C): `/api/v1/alarms`, `/api/v1/alarms_values` and
// `/api/v1/alarm_count` (each body's bytes, healthNorm.json), the alert members of `/api/v1/info`, of a chart's JSON,
// of `/api/v1/alarm_variables` and of a v2 data answer (dataAlerts), and, both stopped, health.log in file order with
// its transition ids as ranks (healthLogRanked). Cases:
//   - `statuses`: `health.transitions`' three alerts through CLEAR, WARNING, CRITICAL, CLEAR at debug level; at each
//     phase every endpoint and member; after the stop also HEALTH's `Alert event for` records;
//   - `flags`: what failing expressions, an empty lookup and missing thresholds leave (healthFlagsConf), the selection
//     tokens of the two listing endpoints, an alert's value by its name;
//   - `delay`: the notification delay's hysteresis: a change outside the last delay, two inside it, one outside;
//   - `repeat`: a repeating alert through WARNING, CRITICAL and CLEAR: `last_repeat`, `times_repeat`, the repeats'
//     records and the unique ids they take;
//   - `texts`: `${family}` and `${label:…}` in `summary` and `info`, at the link and after the chart's labels change;
//   - `obsolete`: an alert of a chart defined obsolete and never collected; `obsolete-long` (PARITY_LONG): a chart
//     collected, then obsolete, 60 s later, and its data view once its alert is removed;
//   - `count-point`: a context's data view whose dimension or instance filter leaves out the chart with the alerts
//     (milestone 10 commit 0, D219 K4), beside `flags`' `alerts=` and `instances=` views (K3);
//   - `off`: the three endpoints and the members with health off.
func TestHealthLoop(t *testing.T) {
	names := []string{"hs_avg", "hs_calc", "hs_max"}
	all := func(i int, h *healthPair) string { return h.get(i, "/api/v1/alarms?all") }
	cases := map[string]healthCase{
		"statuses": {
			conf: healthSigConf,
			grid: healthSigGrid,
			logs: healthLogsDebug,
			sc:   healthValues("hsig.values", "hsig.ctx", []string{"a"}, healthSigPhases...),
			play: func(t *testing.T, h *healthPair) {
				healthPlaySig(t, h, healthSigHold, func(k int) {
					status := healthSigStatus[k]
					at := fmt.Sprintf("phase %d: ", k)
					// what the endpoints without `all` list and count: the raised alerts
					listed, values, raised := map[string]string{}, map[string]string{}, 0
					byStatus := map[string]int{status: len(names)}
					info := map[string]int{"normal": len(names)}
					if status == "WARNING" || status == "CRITICAL" {
						raised = len(names)
						info = map[string]int{strings.ToLower(status): len(names)}
						for _, name := range names {
							listed[name] = status
						}
					}
					for _, name := range names {
						values["hsig.values."+name] = status
					}
					valuesListed := map[string]string{}
					for name, s := range listed {
						valuesListed["hsig.values."+name] = s
					}
					// each alert is linked three times at HEALTH's first pass and then changes status once per phase
					h.asks(t, at,
						healthAsk{"/api/v1/alarms?all", healthBoth(healthAll(status, names...), healthLatest(3*len(names)+len(names)*(k+1)))},
						healthAsk{"/api/v1/alarms", healthWant(listed)},
						healthAsk{"/api/v1/alarms_values?all", healthValuesWant(values)},
						healthAsk{"/api/v1/alarms_values", healthValuesWant(valuesListed)},
						healthAsk{"/api/v1/alarm_count", healthCount(raised)},
						healthAsk{"/api/v1/alarm_count?status=CLEAR", healthCount(byStatus["CLEAR"])},
						healthAsk{"/api/v1/alarm_count?status=warning", healthCount(byStatus["WARNING"])},
						healthAsk{"/api/v1/alarm_count?status=Critical", healthCount(byStatus["CRITICAL"])},
						healthAsk{"/api/v1/alarm_count?status=UNINITIALIZED", healthCount(0)},
						// a status C does not know leaves the default; a context named twice counts twice
						healthAsk{"/api/v1/alarm_count?status=raised", healthCount(raised)},
						healthAsk{"/api/v1/alarm_count?context=hsig.ctx&context=hsig.ctx", healthCount(2 * raised)},
						healthAsk{"/api/v1/alarm_count?status=clear&ctx=other,hsig.ctx", healthCount(byStatus["CLEAR"])},
						healthAsk{"/api/v1/alarm_count?context=other", healthCount(0)},
					)
					h.compareNow(t, at+"/api/v1/info's alarms", func(i int) string { return h.plainMember(i, "/api/v1/info", "alarms") },
						healthIs(fmt.Sprintf(`{"normal":%d,"warning":%d,"critical":%d}`, info["normal"], info["warning"], info["critical"])))
					var alarms []string
					for _, name := range []string{"hs_calc", "hs_max", "hs_avg"} {
						alarms = append(alarms, fmt.Sprintf(`%q:{"id":%q,"status":%q,"units":"things","duration":1}`, name, name, status))
					}
					h.compareNow(t, at+"/api/v1/chart's alarms", func(i int) string {
						return h.plainMember(i, "/api/v1/chart?chart=hsig.values", "alarms")
					}, healthIs("{"+strings.Join(alarms, ",")+"}"))
					// the alerts' dictionary changed nine times (each alert added, deleted and added); the transitions so far;
					// each alert once under its name, and three in the status under the node, the context and the instance
					key := map[string]string{"CLEAR": "cl", "WARNING": "wr", "CRITICAL": "cr"}[status]
					var byName []string
					for _, name := range []string{"hs_calc", "hs_max", "hs_avg"} {
						byName = append(byName, fmt.Sprintf(`{"nm":%q,%q:1}`, name, key))
					}
					h.compareNow(t, at+"the alert members of /api/v2/data", func(i int) string {
						return h.dataAlerts(i, "scope_contexts=hsig.ctx&points=1")
					}, healthIs(fmt.Sprintf("versions.alerts_hard_hash: %d\nversions.alerts_soft_hash: %d\nsummary.alerts: [%s]\n"+
						"summary.nodes[0] mg=%q: {%q:%d}\nsummary.contexts[0] id=\"hsig.ctx\": {%q:%d}\nsummary.instances[0] id=\"hsig.values\": {%q:%d}",
						3*len(names), 3*len(names)+len(names)*(k+1), strings.Join(byName, ","), parentIdentity.MachineGUID, key, len(names), key,
						len(names), key, len(names))))
				})
			},
			after: func(t *testing.T, h *healthPair) {
				// at debug level every entry has its record: three links, the first CLEAR and three transitions per alert
				var want [][]string
				for _, name := range names {
					want = append(want, []string{" alert=" + name + " ", " alert_status=CRITICAL "})
				}
				healthLoopLog(healthLogWant(7*len(names), want...))(t, h)
				h.compareLines(t, "HEALTH's records of the transitions", func(i int) []string {
					return h.threadRecords(t, i, "HEALTH", "["+h.p.Each()[i].Daemon.Hostname+"]: Alert event for [")
				}, func(oracle []string) error {
					// one per status change in an evaluation: the first CLEAR and three transitions per alert
					if len(oracle) != 4*len(names) {
						return fmt.Errorf("%d records, want %d", len(oracle), 4*len(names))
					}
					return nil
				})
			},
		},
		"flags": {
			conf: healthFlagsConf,
			logs: healthLogsDebug,
			sc:   healthValues("hf.values", "hf.ctx", []string{"a"}, map[string]int64{"a": 10}, map[string]int64{"a": 70}),
			play: func(t *testing.T, h *healthPair) {
				h.create(t)
				low := map[string]string{"hf_before": "CLEAR", "hf_base": "CLEAR", "hf_after": "CLEAR", "hf_ping": "CLEAR",
					"hf_pong": "CLEAR", "hf_self": "CLEAR", "hf_calc_only": "UNDEFINED", "hf_unknown": "CLEAR", "hf_nope": "CLEAR",
					"hf_warn_unknown": "CLEAR", "hf_warn_fails": "UNDEFINED", "hf_crit_only": "CLEAR", "hf_crit_fails": "CLEAR",
					"hf_no_clear": "WARNING"}
				h.compareNow(t, "phase 0: /api/v1/alarms?all", func(i int) string { return all(i, h) }, healthWant(low))
				h.release(t, "p1", 1, healthCalcHold)
				high := maps.Clone(low)
				raised := map[string]string{"hf_before": "WARNING", "hf_base": "WARNING", "hf_after": "WARNING",
					"hf_crit_only": "CRITICAL", "hf_no_clear": "WARNING"}
				maps.Copy(high, raised)
				// three links per alert, the first status of each (hf_before's with no value yet, and its own a pass
				// later is the same), then the four changes
				const entries = 14*3 + 14 + 4
				h.asks(t, "phase 1: ",
					healthAsk{"/api/v1/alarms?all", healthBoth(healthWant(high), healthLatest(entries))},
					// the selection: `all` or `all=true`, `active` or `active=true`, the last one wins; anything else is
					// not a selection (web/api/v1/api_v1_alarms.c:5-16)
					healthAsk{"/api/v1/alarms", healthWant(raised)},
					healthAsk{"/api/v1/alarms?all=true", healthWant(high)},
					healthAsk{"/api/v1/alarms?active&all", healthWant(high)},
					healthAsk{"/api/v1/alarms?all&active=true", healthWant(raised)},
					healthAsk{"/api/v1/alarms?all=1", healthWant(raised)},
					healthAsk{"/api/v1/alarms?ALL", healthWant(raised)},
					healthAsk{"/api/v1/alarms_values?all=true", healthHolds(`"hf.values.hf_nope": {`, `"hf.values.hf_base": {`)},
					healthAsk{"/api/v1/alarms_values?all=yes", healthHolds(`"hf.values.hf_base": {`)},
					healthAsk{"/api/v1/alarm_count", healthCount(len(raised))},
					healthAsk{"/api/v1/alarm_count?status=UNDEFINED", healthCount(2)},
				)
				vars := "/api/v1/alarm_variables?chart=hf.values"
				h.compareNow(t, vars+"'s alerts", func(i int) string { return h.plainMember(i, vars, "alerts") },
					healthHolds(`"hf_base":{"value":70,`, `"hf_after":{"value":70,`, `"hf_calc_only":{"value":140,`))
				one := "/api/v1/variable?chart=hf.values&variable=hf_base"
				h.compareNow(t, one, func(i int) string { return h.trace(i, one) }, healthHolds(`"found":true`, `"value":70`))
				h.compareNow(t, "/api/v1/info's alarms", func(i int) string { return h.plainMember(i, "/api/v1/info", "alarms") },
					healthIs(`{"normal":9,"warning":4,"critical":1}`))
				h.compareNow(t, "the alert members of /api/v2/data", func(i int) string {
					return h.dataAlerts(i, "scope_contexts=hf.ctx&after=-4&points=1&options=details,unaligned")
				}, healthHolds(fmt.Sprintf("versions.alerts_hard_hash: %d\nversions.alerts_soft_hash: %d\n", 14*3, entries),
					`{"nm":"hf_calc_only","ot":1}`, `{"cl":7,"wr":4,"cr":1,"ot":2}`, `detailed hf.ctx hf.values: {"hf_`))
				// D219 K3: the same query with a filter. `alerts=` asks each alert of the chart, in the order they were linked
				// (the file's, hf_before first: health/rrdcalc.c:308 appends), its name and then `NAME:STATUS`; the first
				// positive match keeps the instance and the first negative one drops it (query_target.c:684-735). A
				// kept instance counts its alerts in itself, its context and its node; one the filter, `instances=` or
				// `labels=` drops is still listed, counts nothing (:842-848) and has no tree, while the summary's
				// alerts still name its alerts (formatters/jsonwrap-summary-alerts.c:11-48 walks every instance of the
				// query)
				for _, k := range []struct {
					q    string
					kept bool
				}{
					{"alerts=hf_base", true},
					{"alerts=nope", false},
					{"alerts=hf_crit_only:CRITICAL", true},
					{"alerts=hf_crit_only:WARNING", false},
					// hf_before is refused before any other alert is asked
					{"alerts=!hf_before%7C*", false},
					// hf_before matches `*` before hf_base could be refused
					{"alerts=!hf_base%7C*", true},
					{"instances=nomatch", false},
					// the label filter, judged before the alerts (query_target.c:833-845): every label key it names
					// must match (pattern-array.c:46-79). C labels every chart `_collect_plugin` with its plugin, here
					// the fake plugin's file (rrdset-index-id.c:23-26), and none `nolabel`; a word's asterisks are lost
					// on the way into the pattern array (simple_pattern.c:414-420 hands back the word without them,
					// pattern-array.c:112 makes it an exact pattern), so `*` matches nothing there
					{"labels=_collect_plugin:difftest.plugin", true},
					{"labels=_collect_plugin:*", false},
					{"labels=nolabel:x", false},
				} {
					h.compareNow(t, "the alert members of /api/v2/data, "+k.q, func(i int) string {
						return h.dataAlerts(i, "scope_contexts=hf.ctx&after=-4&points=1&options=details,unaligned&"+k.q)
					}, healthFiltered(entries, k.kept))
				}
			},
			after: healthLoopLog(func(oracle []string) error {
				// hf_base, hf_after and hf_crit_only change in the pass that reads 70; hf_before, computed before hf_base, a
				// pass later: its record is the last one
				var changes []string
				for _, l := range oracle {
					for _, name := range []string{"hf_before", "hf_base", "hf_after", "hf_crit_only"} {
						if strings.Contains(l, " alert="+name+" ") && strings.Contains(l, " alert_value_old=CLEAR ") {
							changes = append(changes, name)
						}
					}
				}
				if want := []string{"hf_base", "hf_after", "hf_crit_only", "hf_before"}; !slices.Equal(changes, want) {
					return fmt.Errorf("the changes from CLEAR are of %v, want %v", changes, want)
				}
				return nil
			}),
		},
		"delay": {
			conf: healthDelayConf,
			sc: healthValues("hd.values", "hd.ctx", []string{"a"}, map[string]int64{"a": 10}, map[string]int64{"a": 70},
				map[string]int64{"a": 10}, map[string]int64{"a": 70}, map[string]int64{"a": 10}),
			play: func(t *testing.T, h *healthPair) {
				// state compares the alert once it shows the status and the delay its last change of status took
				state := func(what, status string, delay int) {
					t.Helper()
					h.compareNow(t, what+": /api/v1/alarms?all", func(i int) string { return all(i, h) },
						healthBoth(healthAll(status, "hd_calc"), healthHolds(fmt.Sprintf("\"delay\": %d,\n", delay))))
				}
				h.create(t)
				// the first status is a change upwards, outside any delay
				state("the first status", "CLEAR", 6)
				// more than 6 s (and the two a pass may be late) after it: outside, the delays start again
				up := h.release(t, "p1", 1, 9*time.Second)
				state("up, outside the last delay", "WARNING", 6)
				// 3 s later: inside the 6 s, both delays double; the way down takes its own, 16
				down := h.release(t, "p2", 2, 2*time.Second)
				if down-up > 5 {
					t.Fatalf("harness: the value went down %d s after it went up: the change is no longer inside the delay of 6 s on both sides", down-up)
				}
				state("down, inside the last delay", "CLEAR", 16)
				// 3 s later: inside the 16 s, both double again and stop at the maximum
				h.release(t, "p3", 3, 2*time.Second)
				state("up, inside the last delay", "WARNING", 20)
				// more than 20 s after it: outside
				h.release(t, "p4", 4, 22*time.Second)
				state("down, outside the last delay", "CLEAR", 8)
			},
			// at the default level: a record per change to WARNING and back to CLEAR
			after: healthLoopLog(healthLogWant(4, []string{" alert=hd_calc ", " alert_status=WARNING "}, []string{" alert=hd_calc ", " alert_status=CLEAR "})),
		},
		"repeat": {
			conf: healthRepeatConf,
			// every switch at the same second of a 5 s window: a raised phase lasts 10 s on both sides, which a repeat
			// every 4 s divides into the same count whichever second each side's pass first saw it
			grid: healthSigGrid,
			sc:   healthValues("hr.values", "hr.ctx", []string{"a"}, healthSigPhases...),
			play: func(t *testing.T, h *healthPair) {
				// state compares the alert once it shows the status and at least `repeats` repeats
				state := func(what, status string, repeats int) {
					t.Helper()
					h.compareNow(t, what+": /api/v1/alarms?all", func(i int) string { return all(i, h) },
						healthBoth(healthAll(status, "hr_calc"), func(view string) error {
							m := healthTimesRepeatRe.FindStringSubmatch(view)
							if m == nil {
								return fmt.Errorf("no times_repeat")
							}
							if got, _ := strconv.Atoi(m[1]); got < repeats {
								return fmt.Errorf("times_repeat is %d, want %d or more", got, repeats)
							}
							return nil
						}))
				}
				h.create(t)
				state("CLEAR", "CLEAR", 0)
				h.release(t, "p1", 1, 0)
				state("WARNING, repeated", "WARNING", 1)
				h.release(t, "p2", 2, 6*time.Second)
				state("CRITICAL, repeated", "CRITICAL", 3)
				h.release(t, "p3", 3, 6*time.Second)
				state("CLEAR again", "CLEAR", 4)
			},
			// at the default level: the changes to WARNING, CRITICAL and CLEAR, and a record per repeat
			after: healthLoopLog(func(oracle []string) error {
				if len(oracle) < 5 {
					return fmt.Errorf("%d records, want the three changes and two repeats or more", len(oracle))
				}
				// a repeat's record is its change's again, with the value on both sides of it
				if !slices.ContainsFunc(oracle, func(l string) bool {
					return strings.Contains(l, " alert_value=70 alert_value_old=70 alert_status=WARNING alert_value_old=CLEAR ")
				}) {
					return fmt.Errorf("no record of a repeat of WARNING")
				}
				return nil
			}),
		},
		"texts": func() healthCase {
			sc := healthScenario(healthTextsEmit, "ht.a", "ht.ctx", []string{"a"}, map[string]int64{"a": 10}, map[string]int64{"a": 10})
			v := healthChart(sc)
			v.Family, v.Labels = "fam one", healthTextsLabels
			v.Phases[1].Emit = healthTextsRelabel
			want := map[string]string{"ht_tmpl": "CLEAR", "ht_stop": "CLEAR", "ht_long": "CLEAR", "ht_value": "CLEAR", "ht_plain": "CLEAR"}
			return healthCase{
				conf: healthTextsConf,
				logs: healthLogsDebug,
				sc:   sc,
				play: func(t *testing.T, h *healthPair) {
					h.create(t)
					// six alerts linked three times, and the first CLEAR of the five that run
					h.compareNow(t, "/api/v1/alarms?all", func(i int) string { return all(i, h) }, healthBoth(healthWant(want), healthLatest(23), healthHolds(
						`"summary": "on fam one",`, `"info": "family fam one, kind web one, role ${label:role}, none ${label:nope}.",`,
						// a `$` that starts no token ends the scan
						`"summary": "$x ${family}",`, `"info": "fam one costs $5 on ${family}",`,
						`"summary": "v99",`, `"info": "${label:`+healthTextsL100+`} then fam one",`,
						// the label's value as C stored it; the text without its quotes, and not escaped: no JSON, with its `\s`
						`"summary": "a__family_b/fam one",`, `"info": "say  hi  to <b> & back\slash fam onefam one",`,
						"\"summary\": \"\",\n\t\t\t\"info\": \"\",\n")))
					// the chart's labels change while it is collected: its five alerts are unlinked, linked and CLEAR again
					h.release(t, "p1", 1, healthCalcHold)
					h.compareNow(t, "after the labels changed: /api/v1/alarms?all", func(i int) string { return all(i, h) },
						healthBoth(healthWant(want), healthLatest(38), healthHolds(
							`"info": "family fam one, kind web two, role db, none ${label:nope}.",`,
							`"summary": "${label:`+healthTextsL99+`}",`, `"summary": "${label:tricky}/fam one",`)))
				},
				// at debug level the link entries have records: ht.b's alert, which no endpoint lists, shows its texts
				after: healthLoopLog(healthLogWant(38, []string{" instance=ht.b ", ` alert_summary="on ht" `,
					` alert_info="family ht, kind bee, role ${label:role}, none ${label:nope}." `},
					[]string{" alert=ht_value ", " alert_status=REMOVED alert_value_old=CLEAR ", ` alert_summary="a__family_b/fam one" `},
					[]string{" alert=ht_value ", " alert_status=CLEAR alert_value_old=UNINITIALIZED ", ` alert_summary="${label:tricky}/fam one" `})),
			}
		}(),
		"obsolete": {
			conf: healthObsoleteConf,
			logs: healthLogsDebug,
			sc:   healthScenario(healthObsoleteEmit, "ho.a", "ho.ctx", []string{"a"}, map[string]int64{"a": 10}),
			play: func(t *testing.T, h *healthPair) {
				h.create(t)
				// three links per alert, ho_live's first CLEAR and ho_gone's removal
				h.asks(t, "",
					healthAsk{"/api/v1/alarms?all", healthBoth(healthWant(map[string]string{"ho_live": "CLEAR"}), healthLatest(11))},
					healthAsk{"/api/v1/alarm_count?status=REMOVED", healthCount(0)},
					healthAsk{"/api/v1/alarm_count?status=CLEAR", healthCount(1)},
				)
				// C does not serve an obsolete chart's JSON: its alerts show by name in another chart's variables
				h.compareNow(t, "/api/v1/chart?chart=ho.b", func(i int) string { return h.plain(i, "/api/v1/chart?chart=ho.b") }, healthGone("ho.b"))
				vars := "/api/v1/alarm_variables?chart=ho.a"
				h.compareNow(t, vars+"'s alerts", func(i int) string { return h.plainMember(i, vars, "alerts") },
					healthHolds(`"ho_gone":{"value":null,`, `"ho_rep":{"value":null,`, `"ho_live":{"value":10,`))
				h.compareNow(t, "/api/v1/info's alarms", func(i int) string { return h.plainMember(i, "/api/v1/info", "alarms") },
					healthIs(`{"normal":1,"warning":0,"critical":0}`))
				h.compareNow(t, "the alert members of /api/v2/data", func(i int) string {
					return h.dataAlerts(i, "scope_contexts=ho.ctx&points=1")
				}, healthHolds("versions.alerts_hard_hash: 9\nversions.alerts_soft_hash: 11\nsummary.alerts: [{\"nm\":\"ho_live\",\"cl\":1}]\n",
					`summary.instances[0] id="ho.a": {"cl":1}`))
			},
			// ho_gone's fourth record is its removal, at HEALTH's first pass; ho_rep, which repeats, has its three links
			after: healthLoopLog(healthBothLines(healthLogWant(11,
				[]string{" alert=ho_gone ", " alert_event_id=4 ", " alert_status=REMOVED alert_value_old=UNINITIALIZED ", "lowered from UNINITIALIZED to REMOVED"},
				[]string{" alert=ho_live ", " alert_event_id=4 ", " alert_status=CLEAR "}), func(oracle []string) error {
				if n := len(slices.DeleteFunc(slices.Clone(oracle), func(l string) bool { return !strings.Contains(l, " alert=ho_rep ") })); n != 3 {
					return fmt.Errorf("%d records of ho_rep, want its three links", n)
				}
				return nil
			})),
		},
		"count-point": func() healthCase {
			sc := healthScenario(healthPointEmit, "hp.a", "hp.ctx", []string{"a"}, map[string]int64{"a": 10}, map[string]int64{"a": 70})
			v := healthChart(sc)
			v.Phases[0].Emit, v.Phases[1].Emit = healthPointBlock, healthPointBlock
			return healthCase{
				conf: healthPointConf,
				sc:   sc,
				play: func(t *testing.T, h *healthPair) {
					h.create(t)
					h.release(t, "p1", 1, healthCalcHold)
					// four alerts linked three times each, three first statuses (hp_never stays UNINITIALIZED: its window
					// of a day never has data), then hp.a's two changes
					h.asks(t, "",
						healthAsk{"/api/v1/alarms_values?all", healthValuesWant(map[string]string{"hp.side.hp_tmpl": "CLEAR",
							"hp.a.hp_tmpl": "WARNING", "hp.a.hp_lone": "CRITICAL", "hp.a.hp_never": "UNINITIALIZED"})},
						healthAsk{"/api/v1/alarms?all", healthLatest(17)})
					// the data view of hp.ctx, then of hp.side alone by its dimensions and by the instance filter. The alert
					// counts of an instance are taken before its dimensions are added (query_target.c:847-848), so the
					// instance `scope_dimensions=b` drops for having no `b` (:426-438, :866-870) still counts in its node
					// and its context, while the summary's instances and alerts walk the query's instances only
					// (jsonwrap-summary-alerts.c:11-48). `instances=` leaves hp.a in the query, listed and counted nowhere
					// (:831-848), its alerts named in the summary's. hp_tmpl is counted over both instances, hp_never as
					// `ot` (:669-676); the tree has the alerts at CLEAR or above of the queried instances.
					head := "versions.alerts_hard_hash: 12\nversions.alerts_soft_hash: 17\n"
					all := `summary.alerts: [{"nm":"hp_tmpl","cl":1,"wr":1},{"nm":"hp_lone","cr":1},{"nm":"hp_never","ot":1}]` + "\n"
					node := fmt.Sprintf("summary.nodes[0] mg=%q: ", parentIdentity.MachineGUID)
					four := `{"cl":1,"wr":1,"cr":1,"ot":1}`
					side := `summary.instances[0] id="hp.side": {"cl":1}` + "\n"
					sideTree := `detailed hp.ctx hp.side: {"hp_tmpl":{"st":"CLEAR","vl":10,"un":"things"}}`
					for _, r := range []struct{ q, want string }{
						{"", head + all + node + four + "\n" + `summary.contexts[0] id="hp.ctx": ` + four + "\n" + side +
							`summary.instances[1] id="hp.a": {"wr":1,"cr":1,"ot":1}` + "\n" + sideTree + "\n" +
							`detailed hp.ctx hp.a: {"hp_tmpl":{"st":"WARNING","vl":70,"un":"things"},"hp_lone":{"st":"CRITICAL","vl":70,"un":"things"}}`},
						{"scope_dimensions=b&", head + `summary.alerts: [{"nm":"hp_tmpl","cl":1}]` + "\n" + node + four + "\n" +
							`summary.contexts[0] id="hp.ctx": ` + four + "\n" + side + sideTree},
						{"instances=hp.side&", head + all + node + `{"cl":1}` + "\n" + `summary.contexts[0] id="hp.ctx": {"cl":1}` + "\n" + side +
							`summary.instances[1] id="hp.a": none` + "\n" + sideTree},
					} {
						query := "scope_contexts=hp.ctx&after=-60&points=1&" + r.q + "options=details,unaligned"
						h.compareNow(t, "the alert members of /api/v2/data, "+query, func(i int) string { return h.dataAlerts(i, query) },
							healthIs(r.want))
					}
				},
				// hp.a's two changes, at the default level
				after: healthLoopLog(healthLogWant(2,
					[]string{" instance=hp.a ", " alert=hp_tmpl ", " alert_value=70 alert_value_old=10 alert_status=WARNING alert_value_old=CLEAR "},
					[]string{" instance=hp.a ", " alert=hp_lone ", " alert_value=70 alert_value_old=10 alert_status=CRITICAL alert_value_old=CLEAR "})),
			}
		}(),
		"off": {
			conf: healthSigConf,
			off:  true,
			sc:   healthValues("hsig.values", "hsig.ctx", []string{"a"}, map[string]int64{"a": 70}),
			play: func(t *testing.T, h *healthPair) {
				h.createChart(t)
				off := healthHolds("\"latest_alarm_log_unique_id\": 0,\n\t\"status\": false,\n", "\"alarms\": {\n\n\t}\n}\n")
				none := healthIs("{\n\t\"hostname\": \"" + h.p.Oracle.Hostname + "\",\n\t\"alarms\": {\n\n\t}\n}\n")
				h.asks(t, "",
					healthAsk{"/api/v1/alarms?all", off},
					healthAsk{"/api/v1/alarms", off},
					healthAsk{"/api/v1/alarms_values?all", none},
					healthAsk{"/api/v1/alarms_values", none},
					healthAsk{"/api/v1/alarm_count", healthCount(0)},
					healthAsk{"/api/v1/alarm_count?status=CLEAR", healthCount(0)},
					healthAsk{"/api/v1/alarm_count?status=UNINITIALIZED&context=hsig.ctx", healthCount(0)},
				)
				h.compareNow(t, "/api/v1/chart's alarms", func(i int) string {
					return h.plainMember(i, "/api/v1/chart?chart=hsig.values", "alarms")
				}, healthIs("{}"))
				h.compareNow(t, "/api/v1/info's alarms", func(i int) string { return h.plainMember(i, "/api/v1/info", "alarms") },
					healthIs(`{"normal":0,"warning":0,"critical":0}`))
				h.compareNow(t, "the alert members of /api/v2/data", func(i int) string {
					return h.dataAlerts(i, "scope_contexts=hsig.ctx&after=-4&points=1&options=details,unaligned")
				}, healthHolds("versions.alerts_hard_hash: 0\nversions.alerts_soft_hash: 0\nsummary.alerts: []\n", "summary.instances[0] ",
					"detailed hsig.ctx hsig.values: none"))
			},
			after: healthLoopLog(healthLogWant(0)),
		},
	}
	if os.Getenv("PARITY_LONG") != "" {
		sc := healthScenario(healthObsoleteLongEmit, "ho.a", "ho.ctx", []string{"a"}, map[string]int64{"a": 10}, map[string]int64{"a": 10},
			map[string]int64{"a": 10})
		v := healthChart(sc)
		v.Phases[0].Emit, v.Phases[1].Emit, v.Phases[2].Emit = healthObsoleteLongBlock, healthObsoleteLongBlock, healthObsoleteLongGone
		cases["obsolete-long"] = healthCase{
			conf: healthObsoleteLongConf,
			logs: healthLogsDebug,
			sc:   sc,
			play: func(t *testing.T, h *healthPair) {
				h.create(t)
				// ho.old's second collection: its alerts run
				last := h.release(t, "p1", 1, 2*time.Second)
				clear := map[string]string{"ho_old": "CLEAR", "ho_old_rep": "CLEAR", "ho_live": "CLEAR"}
				h.compareNow(t, "collected: /api/v1/alarms?all", func(i int) string { return all(i, h) },
					healthBoth(healthWant(clear), healthLatest(12)))
				// obsolete: its alerts are no longer listed and its JSON is not served; the alerts keep their values, which
				// show by name in the other chart's variables
				h.release(t, "p2", 2, 2*time.Second)
				live := func(latest int) func(string) error {
					return healthBoth(healthWant(map[string]string{"ho_live": "CLEAR"}), healthLatest(latest))
				}
				vars := "/api/v1/alarm_variables?chart=ho.a"
				alerts := func(i int) string { return h.plainMember(i, vars, "alerts") }
				kept := healthHolds(`"ho_old":{"value":10,`, `"ho_old_rep":{"value":10,`, `"ho_live":{"value":10,`)
				h.compareNow(t, "obsolete: /api/v1/alarms?all", func(i int) string { return all(i, h) }, live(12))
				h.compareNow(t, "obsolete: /api/v1/chart?chart=ho.old", func(i int) string { return h.plain(i, "/api/v1/chart?chart=ho.old") },
					healthGone("ho.old"))
				h.compareNow(t, "obsolete: "+vars+"'s alerts", alerts, kept)
				// more than 60 s after the chart's last collection its alert is removed (one more unique id, no value); the
				// repeating one is not
				time.Sleep(time.Until(time.Unix(last+58, 0)))
				h.compareNow(t, "58 s after the last collection: /api/v1/alarms?all", func(i int) string { return all(i, h) }, live(12))
				h.compareNow(t, "58 s after the last collection: "+vars+"'s alerts", alerts, kept)
				time.Sleep(time.Until(time.Unix(last+61, 0)))
				h.compareNow(t, "removed: /api/v1/alarms?all", func(i int) string { return all(i, h) }, live(13))
				h.compareNow(t, "removed: "+vars+"'s alerts", alerts,
					healthHolds(`"ho_old":{"value":null,`, `"ho_old_rep":{"value":10,`, `"ho_live":{"value":10,`))
				// the obsolete chart's data view (D219 K4's REMOVED clause): the removed alert is counted as `ot`
				// (query_target.c:669-676, jsonwrap-summary-alerts.c:36-41) and left out of the tree (CLEAR or above only)
				removed := "scope_contexts=ho.octx&after=-120&points=1&options=details,unaligned"
				h.compareNow(t, "removed: the alert members of /api/v2/data, "+removed, func(i int) string { return h.dataAlerts(i, removed) },
					healthIs("versions.alerts_hard_hash: 9\nversions.alerts_soft_hash: 13\n"+
						`summary.alerts: [{"nm":"ho_old","ot":1},{"nm":"ho_old_rep","cl":1}]`+"\n"+
						fmt.Sprintf("summary.nodes[0] mg=%q: ", parentIdentity.MachineGUID)+`{"cl":1,"ot":1}`+"\n"+
						`summary.contexts[0] id="ho.octx": {"cl":1,"ot":1}`+"\n"+`summary.instances[0] id="ho.old": {"cl":1,"ot":1}`+"\n"+
						`detailed ho.octx ho.old: {"ho_old_rep":{"st":"CLEAR","vl":10,"un":"things"}}`))
			},
			// the removal takes the alert's value: `nan` in the message
			after: healthLoopLog(healthLogWant(13, []string{" alert=ho_old ", " alert_value=null alert_value_old=10 alert_status=REMOVED alert_value_old=CLEAR ",
				"lowered from CLEAR to REMOVED", "value got from 10.000000 things, to nan things."})),
		}
	}
	runHealthCases(t, cases)
}

// an alert's count of repeats in /api/v1/alarms (health_json.c:92)
var healthTimesRepeatRe = regexp.MustCompile(`"times_repeat": (\d+),`)
