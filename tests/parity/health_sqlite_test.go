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

// TestHealthSQLite (check `health.sqlite`, M9 commit 0, D183): alerts through their phases, then, both agents stopped,
// the health tables of the metadata database as metadata-dump prints them (healthNorm.healthDump: ids rebased,
// transition ids aliased, clocks masked, the side's directories replaced), row for row in rowid order. While the
// phases play only the oracle is judged: the comparison is the files'. Cases:
//   - `dump`: the transition case's three alerts through CLEAR, WARNING, CRITICAL and CLEAR: the rows of raised entries
//     carry the notifier's marks;
//   - `quiet` (M9 commit 5, D205 F1): the quiet rule set (healthQuietConf), whose alerts never go above CLEAR: no
//     notification runs, so the rows are the alert log's alone. Then the unclaimed queue's records of the access log
//     and the store jobs' records of the queued saves, each summed.
func TestHealthSQLite(t *testing.T) {
	names := []string{"hs_avg", "hs_calc", "hs_max"}
	runHealthCases(t, map[string]healthCase{
		"quiet": {
			conf: healthQuietConf,
			logs: healthLogsDebug,
			sc:   healthQuietScenario(),
			play: func(t *testing.T, h *healthPair) {
				healthPlayQuiet(t, h)
				// the candidate's log reaches the oracle's last entries before both stop (the read also gives each
				// side's id bases, which the dump prints the ids by); then the queue's delay
				h.waitCandidate("/api/v1/alarm_log", func(i int) string { return h.transitions(i, "") })
				time.Sleep(healthQueueHold)
			},
			after: healthQuietTables(healthQuietRows, healthQuietMoves, healthQuietStored),
		},
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

// healthQuietRows are the rows the quiet rule set leaves once its three phases played and the queue's delay passed
// (healthQueueHold): a rule and a log row per alert; the 26 entries (healthQuietEntries, and hq_undef's two changes);
// in alert_queue the two rows that are not due for ten minutes (hq_gone's removal and hq_var's UNDEFINED: every
// transition of theirs waits 600 s, sqlite_health.c:83-152); in aclk_queue a row for each of the four other alerts
// (their first CLEAR was due 10 s after it, and HEALTH moved it on at its next pass). Nothing reaches the two tables
// an agent fills only while it talks to the Cloud.
var healthQuietRows = map[string]int{"alert_hash": 6, "health_log": 6, "health_log_detail": healthQuietEntries + 2, "alert_queue": 2,
	"aclk_queue": 4, "alert_version": 0, "alert_hash_cloud": 0}

// healthQuietMoves is what the unclaimed queue's records sum to after the quiet phases and the queue's delay: the
// three first CLEARs (hq_tmpl, hq_undef, hq_delay) in one pass, hq_look's, and hq_undef's return to CLEAR; each moved
// on (hq_gone's and hq_var's rows are not due, and hq_var's would not be moved: a rule without a threshold).
const healthQuietMoves = "processed 5, queued 5"

// healthQuietStored is what the store jobs' records sum to after the quiet phases: the 30 saves the first pass queued,
// two counted for each (healthStored). A first link queues its entry; an unlink and the link after it queue the entry
// they replace and their own: 6 + 12 + 12.
const healthQuietStored = 2 * (6 + 12 + 12)

// healthQuietTables compares what a quiet case left once both agents stopped: the health tables, the oracle's with
// the counts of `want`; the unclaimed queue's records of the access log, summed (healthQueueMoves), the oracle's
// being `moves`: the case's timeline keeps every transition 5 s or more from a due second (healthQuietMoved), so the
// rows a side took are the case's, not its timing's; and the store jobs' records of the queued saves, summed
// (healthStored), the oracle's being `stored`. The case runs at debug level for the last.
func healthQuietTables(want map[string]int, moves string, stored int) func(t *testing.T, h *healthPair) {
	is := func(want string) func(oracle []string) error {
		return func(oracle []string) error {
			if len(oracle) != 1 || oracle[0] != want {
				return fmt.Errorf("want %s", want)
			}
			return nil
		}
	}
	return func(t *testing.T, h *healthPair) {
		t.Helper()
		h.compareLines(t, "the health tables", func(i int) []string { return h.n[i].healthDump(t, h.p.Each()[i].Daemon) },
			healthRowsWant(want))
		h.compareLines(t, "the unclaimed queue's records, summed", func(i int) []string { return h.queueMoves(t, i) }, is(moves))
		h.compareLines(t, "the store jobs' records of the queued saves, summed", func(i int) []string { return h.stored(t, i) },
			is(fmt.Sprintf("stored %d alert transitions", stored)))
	}
}
