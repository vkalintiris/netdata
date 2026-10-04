// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"strings"
	"testing"
	"time"
)

// healthQueueHold is how long the sqlite case waits after the last transition before both agents stop. An unclaimed C
// queues each transition in alert_queue with a due time (a return to CLEAR 10 s later, sqlite_health.c:83-190,
// sqlite_health.h:9-11) and HEALTH moves the due rows to aclk_queue at its next pass (sqlite_aclk_alert.c:728-778): a
// stop inside that delay would compare which pass each side's stop fell in. Past it every row was moved on both.
const healthQueueHold = 12 * time.Second

// TestHealthSQLite (check `health.sqlite`, M9 commit 0, D183): the transition case's three alerts through CLEAR,
// WARNING, CRITICAL and CLEAR, then, both agents stopped, the health tables of the metadata database as metadata-dump
// prints them (healthNorm.healthDump: ids rebased, transition ids aliased, clocks masked, the side's directories
// replaced). While the phases play only the oracle is judged: the comparison is the files'.
func TestHealthSQLite(t *testing.T) {
	names := []string{"hs_avg", "hs_calc", "hs_max"}
	runHealthCases(t, map[string]healthCase{
		"dump": {
			conf: healthSigConf,
			grid: healthSigGrid,
			sc:   healthValues("hsig.values", "hsig.ctx", []string{"a"}, healthSigPhases...),
			play: func(t *testing.T, h *healthPair) {
				healthPlaySig(t, h, healthSigHold, func(k int) {
					h.waitOracle(t, fmt.Sprintf("phase %d: /api/v1/alarms?all", k), func() (string, error) {
						v := h.get(0, "/api/v1/alarms?all")
						return v, healthAll(healthSigStatus[k], names...)(v)
					})
				})
				// the candidate's log reaches the oracle's last entries before both stop (the read also gives each
				// side's id bases, which the dump prints the ids by)
				h.waitCandidate("/api/v1/alarm_log", func(i int) string { return h.transitions(i, "") })
				time.Sleep(healthQueueHold)
			},
			after: func(t *testing.T, h *healthPair) {
				h.compareLines(t, "the health tables", func(i int) []string { return h.n[i].healthDump(t, h.p.Each()[i].Daemon) },
					func(oracle []string) error {
						count := func(table string) int {
							n := 0
							for _, l := range oracle {
								if strings.HasPrefix(l, "row "+table+" ") {
									n++
								}
							}
							return n
						}
						// per alert: a rule, a log row, and at least the three link entries, the first CLEAR and the three
						// transitions
						if rules, logs, details := count("alert_hash"), count("health_log"), count("health_log_detail"); rules != len(names) ||
							logs != len(names) || details < 7*len(names) {
							return fmt.Errorf("%d alert_hash, %d health_log and %d health_log_detail rows for %d alerts", rules, logs, details, len(names))
						}
						// the unclaimed queue: past the hold every transition was moved on, and aclk_queue holds a row
						// per alert (its last one: the rows are unique by host and alert, sqlite_aclk_alert.c:728-778)
						if queued, moved := count("alert_queue"), count("aclk_queue"); queued != 0 || moved != len(names) {
							return fmt.Errorf("%d alert_queue and %d aclk_queue rows for %d alerts, want 0 and %d", queued, moved, len(names), len(names))
						}
						return nil
					})
			},
		},
	})
}
