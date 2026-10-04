// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"slices"
	"strings"
	"testing"
	"time"
)

// healthLinkEmit is the link check's chart that is defined and never collected: `hl.b`, named `hl.bname`, in the
// collected chart's context `hl.ctx`, with units of its own, an update every of 5 s, a module and a label. The
// collected chart is `hl.a`.
const healthLinkEmit = `CHART hl.b bname 'title' 'ub' 'family' 'hl.ctx' line 1000 5 '' '' 'm1'
DIMENSION a '' absolute 1 1
CLABEL kind x 1
CLABEL_COMMIT
`

// healthLinkLabels are the link check's host labels ([host labels] of netdata.conf).
const healthLinkLabels = "    tier = prod\n"

// healthLinkExtra leaves hl_off out.
const healthLinkExtra = "    enabled alarms = !hl_off *\n"

// healthLinkRule is a rule of the link check: C links it and never runs it. Its lookup reads a day, and a rule is
// runnable only once its chart's data began that long ago (health_event_loop.c:173-187): the alert stays
// UNINITIALIZED with no value on both sides, whatever the loop does. `every` comes after `lookup`, which sets the
// rule's frequency to its window's length (health_config.c:192, :329).
func healthLinkRule(kind, name, on string, more ...string) string {
	rule := fmt.Sprintf("%s: %s\non: %s\nlookup: average -1d of a\nevery: 1s\n", kind, name, on)
	for _, l := range more {
		rule += l + "\n"
	}
	return rule + "\n"
}

// The rules of `recheck`, and of `match` with healthLinkMore.
var (
	// a template: every chart of the context
	healthLinkTmpl = healthLinkRule("template", "hl_tmpl", "hl.ctx")
	// two rules of one name, the template first: on a chart both match, the alarm is linked (per name C tries the alarm
	// rules of a chain, then its templates: health_prototypes.c:625-645, and the first to make the key wins)
	healthLinkChain = healthLinkRule("template", "hl_chain", "hl.ctx", "units: the template") +
		healthLinkRule("alarm", "hl_chain", "hl.a", "units: the alarm")
	// a rule whose text stops parsing once `green` is written into it (`5 + 5ish`): the rule loads, and each link's
	// copy parses the text again (health_prototypes.c:606-608), logs the evaluator's record and has no warning
	healthLinkReparse = healthLinkRule("alarm", "hl_reparse", "hl.a", "green: 5", "warn: $green + $greenish")
)

// healthLinkMore are the other rules of `match`: what decides whether a rule is linked to a chart.
var healthLinkMore = strings.Join([]string{
	// an alarm by the chart's id, and by its name
	healthLinkRule("alarm", "hl_id", "hl.a", "units: by id"),
	healthLinkRule("alarm", "hl_name", "hl.bname"),
	// a chain whose first rule is off (`!*` turns the rule off as it is read: health_config.c:540-544)
	healthLinkRule("template", "hl_dis", "hl.ctx", "os: !*", "units: the first rule"),
	healthLinkRule("template", "hl_dis", "hl.ctx", "units: the second rule"),
	// `[health] enabled alarms`
	healthLinkRule("alarm", "hl_off", "hl.a"),
	// the host's labels: its name, its operating system (a `!` is the value's), a label of netdata.conf
	healthLinkRule("alarm", "hl_hosts_yes", "hl.a", "hosts: parity-*"),
	healthLinkRule("alarm", "hl_hosts_no", "hl.a", "hosts: other-*"),
	healthLinkRule("alarm", "hl_os_yes", "hl.a", "os: linux"),
	healthLinkRule("alarm", "hl_os_no", "hl.a", "os: !linux *"),
	healthLinkRule("alarm", "hl_label_yes", "hl.a", "host labels: tier=prod"),
	healthLinkRule("alarm", "hl_label_no", "hl.a", "host labels: tier=dev"),
	// the chart's labels: its own, its plugin, its module
	healthLinkRule("template", "hl_chart_label", "hl.ctx", "chart labels: kind=x"),
	healthLinkRule("template", "hl_plugin_yes", "hl.ctx", "plugin: difftest.plugin"),
	healthLinkRule("template", "hl_plugin_no", "hl.ctx", "plugin: other.plugin"),
	healthLinkRule("template", "hl_module", "hl.ctx", "module: m1"),
}, "")

// healthLinkAlarms is the `alarms` member of a chart's JSON (web/api/formatters/rrdset2json.c:79-99) for alerts that
// were linked and never run: each `name units` of `alerts`, in the order they were linked. `duration` is the rule's
// `every`, on hl.b too, whose update every is longer (C does not raise it to the chart's: rrdcalc.c:422-430).
func healthLinkAlarms(alerts ...string) string {
	var out []string
	for _, a := range alerts {
		name, units, _ := strings.Cut(a, " ")
		out = append(out, fmt.Sprintf(`%q:{"id":%q,"status":"UNINITIALIZED","units":%q,"duration":1}`, name, name, units))
	}
	return "{" + strings.Join(out, ",") + "}"
}

// healthLinkAlerts is the `alerts` member of /api/v1/alarm_variables (health/rrdvar.c:259-317) asked for hl.a (`own`
// its alerts, `other` hl.b's) or for hl.b: the host's alerts by name, the names in the order they were first linked
// (hl.b's, the chart created first, then hl.a's), each with the chart of the alert that shares the most labels with
// the asked chart and that count: the asked chart's own alert (hl.a has two labels, hl.b three), else the other
// chart's (one label in common, the plugin's). No alert has a value.
func healthLinkAlerts(asked string, a, b []string) string {
	own, other, labels := a, b, 2
	if asked == "hl.b" {
		own, other, labels = b, a, 3
	}
	name := func(alert string) string { return strings.SplitN(alert, " ", 2)[0] }
	has := func(list []string, n string) bool {
		return slices.ContainsFunc(list, func(alert string) bool { return name(alert) == n })
	}
	var out []string
	for _, alert := range append(slices.Clone(b), a...) {
		n := name(alert)
		entry := fmt.Sprintf(`%q:{"value":null,"instance":"hl.b","context":"hl.ctx","score":1}`, n)
		switch {
		case slices.ContainsFunc(out, func(e string) bool { return strings.HasPrefix(e, fmt.Sprintf("%q:", n)) }):
			continue
		case has(own, n):
			entry = fmt.Sprintf(`%q:{"value":null,"instance":%q,"context":"hl.ctx","score":%d}`, n, asked, labels)
		case has(other, n) && asked == "hl.b":
			entry = fmt.Sprintf(`%q:{"value":null,"instance":"hl.a","context":"hl.ctx","score":1}`, n)
		}
		out = append(out, entry)
	}
	return "{" + strings.Join(out, ",") + "}"
}

// linkRecords are side i's records of the alerts' links: the evaluator's, on HEALTH, about a text it could not parse
// again.
func (h *healthPair) linkRecords(t *testing.T, i int) []string {
	t.Helper()
	return h.threadRecords(t, i, "HEALTH", "failed to parse expression ")
}

// TestHealthLink (check `health.link`, M9 commit 3, D194 F1): what linking alone produces, on rules C links and never
// runs (healthLinkRule), so no evaluation shows on either side. Two charts of the fake plugin in one context: `hl.b`,
// defined and never collected, then `hl.a`, collected. Compared: the `alarms` member of each chart's JSON (the alerts
// in the order they were linked, each UNINITIALIZED, with the rule's units or the chart's), `/api/v1/charts`'
// `alarms_count`, `/api/v1/info`'s `alarms` (an alert of a chart that was never collected is not counted), the
// `alerts` member of `/api/v1/alarm_variables`, and, both stopped, HEALTH's records of the links (linkRecords). Cases:
//   - `match`: which rule is linked to which chart (healthLinkMore): by id, by name, by context, the order within a
//     chain, a rule that is off, `enabled alarms`, host labels and chart labels that match and that do not;
//   - `recheck`: four rules; after the first comparisons `reload-labels` on both sides, which has HEALTH unlink and
//     link every alert of the host again at its next pass (database/rrdhost-labels.c:265,
//     health_event_loop.c:211-256): the oracle's alert log must show it, the members are compared again, and the
//     records hold one more link.
func TestHealthLink(t *testing.T) {
	sc := healthScenario(healthLinkEmit, "hl.a", "hl.ctx", []string{"a"}, map[string]int64{"a": 10})
	// members compares what the API shows of the links; `a` and `b` are the two charts' alerts
	members := func(t *testing.T, h *healthPair, when string, a, b []string) {
		t.Helper()
		member := func(what, path, name, want string) {
			t.Helper()
			h.compareNow(t, when+what, func(i int) string { return h.plainMember(i, path, name) }, healthIs(want))
		}
		for _, c := range []struct {
			chart string
			want  []string
		}{{"hl.a", a}, {"hl.b", b}} {
			path := "/api/v1/chart?chart=" + c.chart
			member(path+"'s alarms", path, "alarms", healthLinkAlarms(c.want...))
		}
		member("/api/v1/charts' alarms_count", "/api/v1/charts", "alarms_count", fmt.Sprint(len(a)+len(b)))
		// an alert counts once its chart was collected: hl.a's
		member("/api/v1/info's alarms", "/api/v1/info", "alarms", fmt.Sprintf(`{"normal":%d,"warning":0,"critical":0}`, len(a)))
		for _, chart := range []string{"hl.a", "hl.b"} {
			path := "/api/v1/alarm_variables?chart=" + chart
			member(path+"'s alerts", path, "alerts", healthLinkAlerts(chart, a, b))
		}
	}
	// records compares HEALTH's records of the links after the stop: one per link of hl_reparse
	records := func(links int) func(t *testing.T, h *healthPair) {
		return func(t *testing.T, h *healthPair) {
			h.compareLines(t, "HEALTH's records of the links", func(i int) []string { return h.linkRecords(t, i) },
				func(oracle []string) error {
					for _, l := range oracle {
						if !strings.Contains(l, `failed to parse expression '5 + 5ish'`) {
							return fmt.Errorf("a record that is not hl_reparse's")
						}
					}
					if len(oracle) != links {
						return fmt.Errorf("%d records, want %d: one per link of hl_reparse", len(oracle), links)
					}
					return nil
				})
		}
	}
	matchA := []string{"hl_tmpl units", "hl_chain the alarm", "hl_reparse units", "hl_id by id", "hl_dis the second rule",
		"hl_hosts_yes units", "hl_os_yes units", "hl_label_yes units", "hl_plugin_yes units"}
	matchB := []string{"hl_tmpl ub", "hl_chain the template", "hl_name ub", "hl_dis the second rule", "hl_chart_label ub",
		"hl_plugin_yes ub", "hl_module ub"}
	recheckA := []string{"hl_tmpl units", "hl_chain the alarm", "hl_reparse units"}
	recheckB := []string{"hl_tmpl ub", "hl_chain the template"}
	runHealthCases(t, map[string]healthCase{
		"match": {
			conf:       healthLinkTmpl + healthLinkChain + healthLinkReparse + healthLinkMore,
			extra:      healthLinkExtra,
			hostLabels: healthLinkLabels,
			sc:         sc,
			play: func(t *testing.T, h *healthPair) {
				h.create(t)
				members(t, h, "", matchA, matchB)
			},
			// HEALTH's first pass links the host's alerts, then unlinks and links them for the label recheck the start
			// left pending (healthPair.create): two links of hl_reparse
			after: records(2),
		},
		"recheck": {
			conf: healthLinkTmpl + healthLinkChain + healthLinkReparse,
			sc:   sc,
			play: func(t *testing.T, h *healthPair) {
				h.create(t)
				members(t, h, "", recheckA, recheckB)
				for _, s := range h.p.Each() {
					if r := runCLI(t, s.Daemon, "reload-labels"); r.Exit != 0 {
						t.Fatalf("%s: reload-labels: exit %d: %s", s.Role, r.Exit, r.Stderr)
					}
				}
				// the oracle unlinked and linked every alert again: its log holds five entries per alert, hl_reparse's
				// (one alert of its name) through REMOVED and UNINITIALIZED three times
				h.waitOracle(t, "the alert log after reload-labels", func() (string, error) {
					got := h.transitions(0, "")
					lines := strings.Split(got, "\n")
					relinked := []string{"REMOVED", "UNINITIALIZED", "REMOVED", "UNINITIALIZED", "REMOVED", "UNINITIALIZED"}
					if seq := healthSequence(lines, "hl_reparse"); !slices.Equal(seq, relinked) {
						return got, fmt.Errorf("hl_reparse went through %v, want %v", seq, relinked)
					}
					if want := 5 * (len(recheckA) + len(recheckB)); len(lines) != want {
						return got, fmt.Errorf("%d entries, want %d", len(lines), want)
					}
					return got, nil
				})
				members(t, h, "after reload-labels: ", recheckA, recheckB)
				// the candidate gets the bounded wait to write the relink's record before both stop
				for end := time.Now().Add(healthCandidateWait); len(h.linkRecords(t, 1)) < 3 && time.Now().Before(end); {
					time.Sleep(250 * time.Millisecond)
				}
			},
			after: records(3),
		},
	})
}
