// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"slices"
	"strings"
	"testing"
	"time"
)

// healthSigConf are the alerts of the transition cases, all on the fake plugin's chart `hsig.values` (dimension `a`),
// evaluated every second, WARNING above 50 and CRITICAL above 90: the dimension's last value (one point, no window),
// the maximum over three seconds ending at the last stored point (`unaligned`: a step shows at once and stays three
// seconds), and the average over five seconds, whose window C aligns to its length (query-window.c:322-331), so a
// change shows at the next 5 s boundary, mixed with the old value when the boundary cuts the step.
const healthSigConf = `# the health checks' alerts on hsig.values
 alarm: hs_calc
    on: hsig.values
  calc: $a
 every: 1s
  warn: $this > 50
  crit: $this > 90
 units: things
  info: the last value of a

 alarm: hs_max
    on: hsig.values
lookup: max -3s unaligned of a
 every: 1s
  warn: $this > 50
  crit: $this > 90
 units: things
  info: the maximum of a over 3 seconds

 alarm: hs_avg
    on: hsig.values
lookup: average -5s of a
 every: 1s
  warn: $this > 50
  crit: $this > 90
 units: things
  info: the average of a over 5 aligned seconds
`

// healthSigHold is how long each value of the transition cases is held: an aligned lookup over W seconds shows a held
// value whole only in a window that starts after the change, up to W-1 s later, and both agents need a pass after it:
// 2W+3 s for hs_avg's W of 5 (plan §5.3).
const healthSigHold = 13 * time.Second

// healthSigPhases are the values of `a` the transition cases play: CLEAR, WARNING, CRITICAL, CLEAR.
var healthSigPhases = []map[string]int64{{"a": 10}, {"a": 70}, {"a": 95}, {"a": 10}}

// healthSigStatus are the statuses each phase ends in.
var healthSigStatus = []string{"CLEAR", "WARNING", "CRITICAL", "CLEAR"}

// healthAll is a guard on /api/v1/alarms?all: every named alert has the status.
func healthAll(status string, names ...string) func(string) error {
	want := map[string]string{}
	for _, n := range names {
		want[n] = status
	}
	return healthWant(want)
}

// healthPlaySig plays the transition phases: the chart, then each value held healthSigHold; `phase` runs once the
// phase's values are being collected (phase 0: the chart exists).
func healthPlaySig(t *testing.T, h *healthPair, hold time.Duration, phase func(k int)) {
	t.Helper()
	h.create(t)
	phase(0)
	for k := 1; k < len(healthSigPhases); k++ {
		h.release(t, fmt.Sprintf("p%d", k), k, hold)
		phase(k)
	}
}

// TestHealthTransitions (check `health.transitions`, M9 commit 0, D183): three alerts on one chart of the fake plugin
// through CLEAR, WARNING, CRITICAL and CLEAR, each value switched on both agents at the same second and held 13 s. At
// each phase `/api/v1/alarms?all` is compared (the hand-built body's bytes, healthNorm.json), then each alert's
// transitions with their values from `/api/v1/alarm_log`, that answer's whole body, and, both stopped, health.log in
// file order.
func TestHealthTransitions(t *testing.T) {
	names := []string{"hs_avg", "hs_calc", "hs_max"}
	runHealthCases(t, map[string]healthCase{
		"basic": {
			conf: healthSigConf,
			sc:   healthValues("hsig.values", "hsig.ctx", []string{"a"}, healthSigPhases...),
			play: func(t *testing.T, h *healthPair) {
				healthPlaySig(t, h, healthSigHold, func(k int) {
					h.compareNow(t, fmt.Sprintf("phase %d: /api/v1/alarms?all", k),
						func(i int) string { return h.get(i, "/api/v1/alarms?all") }, healthAll(healthSigStatus[k], names...))
				})
				h.compareNow(t, "the alert log's transitions", func(i int) string { return h.transitions(i, "") },
					func(oracle string) error {
						lines := strings.Split(oracle, "\n")
						// each alert is linked three times at HEALTH's first pass (healthPair.create); then a step shows
						// whole in the last value and in the unaligned maximum
						step := []string{"REMOVED", "UNINITIALIZED", "REMOVED", "UNINITIALIZED", "CLEAR", "WARNING", "CRITICAL", "CLEAR"}
						for _, name := range []string{"hs_calc", "hs_max"} {
							if got := healthSequence(lines, name); !slices.Equal(got, step) {
								return fmt.Errorf("%s went through %v, want %v", name, got, step)
							}
						}
						// an aligned window that cuts a step holds a value between the two: hs_avg may pass through
						// WARNING again on its way down (or up); the rest of its path is fixed
						got := healthSequence(lines, "hs_avg")
						if len(got) < len(step) || !slices.Equal(got[:6], step[:6]) || !slices.Contains(got, "CRITICAL") || got[len(got)-1] != "CLEAR" {
							return fmt.Errorf("hs_avg went through %v", got)
						}
						return nil
					})
				// then every member of every entry, in the log's order
				h.compareNow(t, "/api/v1/alarm_log", func(i int) string { return h.get(i, "/api/v1/alarm_log") },
					func(oracle string) error {
						if !strings.HasPrefix(oracle, "HTTP 200, ") {
							return fmt.Errorf("answered %q", strings.SplitN(oracle, "\n", 2)[0])
						}
						return nil
					})
			},
			after: func(t *testing.T, h *healthPair) {
				h.compareLines(t, "health.log", func(i int) []string { return h.n[i].healthLog(t, h.p.Each()[i].Daemon) },
					func(oracle []string) error {
						// at the default level: every transition to WARNING, to CRITICAL and back to CLEAR
						for _, name := range names {
							n := 0
							for _, l := range oracle {
								if strings.Contains(l, " alert="+name+" ") {
									n++
								}
							}
							if n < 3 {
								return fmt.Errorf("%d records of %s, want at least 3", n, name)
							}
						}
						return nil
					})
			},
		},
	})
}
