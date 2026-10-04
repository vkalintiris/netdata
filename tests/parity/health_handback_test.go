// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// healthAgainEntries is what the second run of a hand-back case adds to the 26 entries of the first
// (healthQuietEntries + 2), on a C agent:
//   - at the load, a REMOVED entry for each alert whose last saved transition is not REMOVED
//     (sqlite_health.c:528-615): two, hq_undef's and hq_look's. The four others' rows name, as their alert's last
//     transition, the REMOVED entry of the first pass's unlink: the three link entries of HEALTH's first pass are
//     stored by the metadata thread after the pass's own entries, and each insert takes the alert's row
//     (sqlite_health.c:281-287, sqlite_metadata.c:2526-2536); only a later transition names itself again;
//   - the three links of each of the six alerts at HEALTH's first pass: 18;
//   - hq_gone's removal and the first status of the four alerts on a value: 5; then hq_look's first CLEAR: 1.
const healthAgainEntries = 2 + 18 + 5 + 1

// healthAgainStored is what the second run's store jobs' records sum to (healthStored): 30 queued saves, as in the
// first run, and one more for each of the two entries the load logged, which the alert's first link replaces (the entries
// the four other alerts loaded were replaced in the first run: a link marks, and saves, only an entry no other one
// replaced yet, health_log.c:277-293).
const healthAgainStored = healthQuietStored + 2*2

// an entry of the alert log that takes an alert from CLEAR to REMOVED: none of the quiet alerts is removed while it
// is CLEAR, so these are the entries a load logs
var healthLoadedRe = regexp.MustCompile(`"status":"REMOVED",\s*"old_status":"CLEAR",`)

// healthLoaded is a guard on a view of `/api/v1/alarm_log`: n entries were logged by a load.
func healthLoaded(n int) func(string) error {
	return func(view string) error {
		if got := len(healthLoadedRe.FindAllString(view, -1)); got != n {
			return fmt.Errorf("%d entries from CLEAR to REMOVED, want %d", got, n)
		}
		return nil
	}
}

// A load's entry for an alert whose last entry was notified, in a rendered alert log: the REMOVED row a start injects
// copies that entry's flags and the second its notifier ran (sqlite_health.c:447-452), so the alert log shows a
// REMOVED entry that was "executed". Between the two members an entry holds no closing brace but the `{run}/` of a
// path.
var healthLoadedNotifiedRe = regexp.MustCompile(`"exec_run":T,(?:[^}]|\}/)*?"status":"REMOVED",\s*"old_status":"WARNING",`)

// healthNotifiedCase is a hand-back case with a notified alert (M9 commit 6, D208): hs_calc alone. The first run
// ends in WARNING, with one call; the second finds the value still at 70, then 10. At its first pass C logs a REMOVED
// entry for the alert, with the flags of the WARNING entry it replaces, and asks the table for the alert's last
// executed entry when the alert's first status comes: that is now the REMOVED one, which is not WARNING, so the
// WARNING is notified a second time (health_notifications.c:414-425, sqlite_health.c:1016-1061); its CLEAR is the
// third call. Compared: the transcripts of both runs, the alert log whole and `/api/v1/alarms?all`, and, both
// stopped, the health tables, the unclaimed queue's records and the store jobs' records summed, and each load's
// record. The values are held as the quiet cases hold theirs: no transition near a second at which a row of the
// unclaimed queue is due.
func healthNotifiedCase(bins [2][2]Role) healthCase {
	const chart, context = "hsig.values", "hsig.ctx"
	sc := healthValues(chart, context, []string{"a"}, map[string]int64{"a": 10}, map[string]int64{"a": 70})
	sc.Starts = append(sc.Starts, plugin.Start{Steps: []plugin.Step{{WaitFile: "again"},
		{Values: &plugin.Values{Chart: chart, Context: context, Dims: []string{"a"},
			Phases: []plugin.Phase{{Set: map[string]int64{"a": 70}, Until: "again1"}, {Set: map[string]int64{"a": 10}}}}}}})
	// the first run's entries: the three links, the first CLEAR, the WARNING; the second's: the load's, the three
	// links, the WARNING again; then the CLEAR
	const first, second = 5, 5
	return healthCase{
		conf:   healthCalcConf,
		dbMode: "alloc",
		logs:   healthLogsDebug,
		sc:     sc,
		bins:   bins,
		play: func(t *testing.T, h *healthPair) {
			h.create(t)
			h.processed(t, "the first run, CLEAR", "hs_calc", "CLEAR")
			h.release(t, "p1", 1, healthQuietMoved)
			h.compareNow(t, "the first run: the notifier's calls", h.transcript(t), healthCallsWant(1,
				healthCallFor("hs_calc", "WARNING", 1, map[int]string{10: "CLEAR"}), healthCallsEnded(0)))
			h.compareNow(t, "the first run: the alert log's notification state", h.execView, healthBoth(healthLogLines(first),
				healthHas("hs_calc: CLEAR->WARNING exec_run=T exec_code=0 exec_failed=false processed=true updated=false")))
			// the WARNING's row of the queue is moved on before the stop
			time.Sleep(healthQueueHold)
		},
		again: func(t *testing.T, h *healthPair) {
			time.Sleep(2 * time.Second)
			h.release(t, "again", 0, 0)
			h.settle(t, h.n, "")
			log := func(i int) string { return h.get(i, "/api/v1/alarm_log") }
			h.processed(t, "after the second start", "hs_calc", "WARNING")
			// the same status as the first run's last, notified again: the call's old status is the link's
			h.compareNow(t, "after the second start: the notifier's calls", h.transcript(t), healthCallsWant(2,
				healthCallFor("hs_calc", "WARNING", 2, map[int]string{10: "UNINITIALIZED"}), healthCallsEnded(0)))
			h.compareNow(t, "after the second start: /api/v1/alarm_log", log, healthBoth(healthLogEntries(first+second),
				func(view string) error {
					if n := len(healthLoadedNotifiedRe.FindAllString(view, -1)); n != 1 {
						return fmt.Errorf("%d entries from WARNING to REMOVED with `exec_run` set, want the load's one", n)
					}
					return nil
				}))
			h.compareNow(t, "after the second start: /api/v1/alarms?all", func(i int) string { return h.get(i, "/api/v1/alarms?all") },
				healthBoth(healthAll("WARNING", "hs_calc"), healthLatest(first+second)))
			h.release(t, "again1", 1, healthCalcHold)
			h.compareNow(t, "CLEAR: the notifier's calls", h.transcript(t), healthCallsWant(3,
				healthCallFor("hs_calc", "CLEAR", 1, map[int]string{10: "WARNING"}), healthCallsEnded(0)))
			h.compareNow(t, "CLEAR: /api/v1/alarm_log", log, healthLogEntries(first+second+1))
			// the CLEAR's row of the queue is due 10 s after it
			time.Sleep(healthQueueHold)
		},
		after: func(t *testing.T, h *healthPair) {
			healthQuietTables(map[string]int{"alert_hash": 1, "health_log": 1, "health_log_detail": first + second + 1,
				"alert_queue": 0, "aclk_queue": 1, "alert_version": 0, "alert_hash_cloud": 0}, healthNotifiedMoves, healthNotifiedStored)(t, h)
			h.compareLines(t, "HEALTH's records of the load", func(i int) []string {
				return h.threadRecords(t, i, "HEALTH", "["+h.p.Each()[i].Daemon.Hostname+"]: Table health_log, loaded ")
			}, func(oracle []string) error {
				if len(oracle) != 2 || !strings.Contains(oracle[0], ", loaded 0 alarm entries, errors in 0 entries.") ||
					!strings.Contains(oracle[1], ", loaded 1 alarm entries, errors in 0 entries.") {
					return fmt.Errorf("want a load of no entry, then one of 1")
				}
				return nil
			})
		},
	}
}

// What a notified hand-back case leaves in its logs, summed over both runs: the rows of the unclaimed queue the
// passes took and moved on, and the queued saves the store jobs took (healthQueueMoves, healthStored).
const (
	healthNotifiedMoves  = "processed 4, queued 4"
	healthNotifiedStored = 2 * (5 + 6)
)

// healthLogLines is a guard on a view of one line per entry (execView): it holds n entries.
func healthLogLines(n int) func(string) error {
	return func(view string) error {
		if got := len(strings.Split(view, "\n")); got != n {
			return fmt.Errorf("%d entries, want %d", got, n)
		}
		return nil
	}
}

// TestHealthHandBack (check `health.handback`, M9 commit 5, D205 F1): what an agent that starts on a cache with an
// alert log does with it. The quiet rule set (healthQuietConf: no notification) plays its three phases, the unclaimed
// queue's delay passes and both agents stop; then each side's run directory is started again, in `alloc` mode, so
// HEALTH's first pass waits for the chart's data in both runs. At that pass C loads each alert's last saved entry,
// after it logged a REMOVED entry for those that were not removed, and the alerts linked afterwards go on with their
// alarm ids and their event ids, and the log with its unique ids (sqlite_health.c:700-882). Compared once the second
// run settled: `/api/v1/alarm_log` whole (the first run's entries, the load's, the second run's), `/api/v1/alarms?all`,
// both again after hq_undef changed once more, and, both stopped, the health tables, the unclaimed queue's records
// and the store jobs' records summed, and HEALTH's record of each load. Every id prints by the first run's bases: ids
// that do not continue show. Cases, by each side's binary in the two runs:
//   - `restart`: the oracle and the candidate, each again on the cache it wrote;
//   - `c-after-rust`: the second run is C's on both sides: C on the candidate's cache beside C on its own;
//   - `rust-after-c`: the first run is C's on both sides: the candidate on a C cache beside C on one.
//
// Two more cases have an alert that was notified before the stop (healthNotifiedCase; M9 commit 6, D208):
// `notified` (each side again on its own cache) and `c-after-rust-notified` (the second run is C's on both sides).
func TestHealthHandBack(t *testing.T) {
	cases := map[string]healthCase{
		"notified":              healthNotifiedCase([2][2]Role{{Oracle, Candidate}, {Oracle, Candidate}}),
		"c-after-rust-notified": healthNotifiedCase([2][2]Role{{Oracle, Candidate}, {Oracle, Oracle}}),
	}
	for name, bins := range map[string][2][2]Role{
		"restart":      {{Oracle, Candidate}, {Oracle, Candidate}},
		"c-after-rust": {{Oracle, Candidate}, {Oracle, Oracle}},
		"rust-after-c": {{Oracle, Oracle}, {Oracle, Candidate}},
	} {
		// the second start of the fake plugin: the same two charts, created at the `again` release, then 10 and 70
		sc := healthQuietScenario()
		sc.Starts = append(sc.Starts, plugin.Start{Steps: []plugin.Step{{WaitFile: "again"}, {Emit: healthQuietEmit},
			{Values: &plugin.Values{Chart: healthQuietChart, Context: "hq.ctx", Dims: []string{"a"},
				Phases: []plugin.Phase{{Set: map[string]int64{"a": 10}, Until: "again1"}, {Set: map[string]int64{"a": 70}}}}}}})
		cases[name] = healthCase{
			conf:   healthQuietConf,
			dbMode: "alloc",
			logs:   healthLogsDebug,
			sc:     sc,
			bins:   bins,
			play: func(t *testing.T, h *healthPair) {
				healthPlayQuiet(t, h)
				// each side's log holds its last entries, and every due row of the queue was moved on, before the stop
				h.waitCandidate("/api/v1/alarm_log", func(i int) string { return h.transitions(i, "") })
				time.Sleep(healthQueueHold)
			},
			again: func(t *testing.T, h *healthPair) {
				// the charts again, at the same second on both sides: HEALTH's first pass loads the log and links the alerts
				time.Sleep(2 * time.Second)
				h.release(t, "again", 0, 0)
				h.settle(t, h.n, "")
				const first = healthQuietEntries + 2
				all := func(i int) string { return h.get(i, "/api/v1/alarms?all") }
				log := func(i int) string { return h.get(i, "/api/v1/alarm_log") }
				// The alert log first: a candidate that serves none, or that did not go on from the cache, fails here.
				// Settled: of the first run's entries nine are processed (healthLogAsks), the two the load logged took
				// their flags from the entries they replace, and of the second run's hq_look's third link and the six
				// statuses (hq_delay's 6 s after it was logged).
				h.compareNow(t, "after the second start: /api/v1/alarm_log", log, healthBoth(healthLogEntries(first+healthAgainEntries),
					healthTimes(`"processed":true,`, 9+2+7), healthLoaded(2),
					healthLacks(`"status":"WARNING"`, `"status":"CRITICAL"`, `"exec_run":T`)))
				h.compareNow(t, "after the second start: /api/v1/alarms?all", all,
					healthBoth(healthWant(healthQuietWant("CLEAR")), healthLatest(first+healthAgainEntries)))
				// hq_undef changes once more, after every due row of the queue was moved on (healthQuietMoved): its event
				// ids go on from the entry the load logged, and its transition makes a row of the queue again
				h.release(t, "again1", 1, healthQuietMoved)
				h.compareNow(t, "hq_undef UNDEFINED again: /api/v1/alarms?all", all,
					healthBoth(healthWant(healthQuietWant("UNDEFINED")), healthLatest(first+healthAgainEntries+1)))
				h.compareNow(t, "hq_undef UNDEFINED again: /api/v1/alarm_log", log,
					healthBoth(healthLogEntries(first+healthAgainEntries+1), healthTimes(`"processed":true,`, 9+2+8)))
			},
			// In alert_queue the rows that are not due for ten minutes: hq_gone's, hq_var's and hq_undef's last change; in
			// aclk_queue the rows of hq_tmpl, hq_undef, hq_delay and hq_look, as after the first run, with the second run's
			// entries. The queue's records: the first run's five rows, then the three first CLEARs of the second run and
			// hq_look's.
			after: func(t *testing.T, h *healthPair) {
				healthQuietTables(map[string]int{"alert_hash": 6, "health_log": 6,
					"health_log_detail": healthQuietEntries + 2 + healthAgainEntries + 1, "alert_queue": 3, "aclk_queue": 4,
					"alert_version": 0, "alert_hash_cloud": 0}, "processed 9, queued 9", healthQuietStored+healthAgainStored)(t, h)
				// what each start's load found (a debug record, sqlite_health.c:873-875): nothing, then an entry per alert
				h.compareLines(t, "HEALTH's records of the load", func(i int) []string {
					return h.threadRecords(t, i, "HEALTH", "["+h.p.Each()[i].Daemon.Hostname+"]: Table health_log, loaded ")
				}, func(oracle []string) error {
					if len(oracle) != 2 || !strings.Contains(oracle[0], ", loaded 0 alarm entries, errors in 0 entries.") ||
						!strings.Contains(oracle[1], fmt.Sprintf(", loaded %d alarm entries, errors in 0 entries.", len(healthQuietNames))) {
						return fmt.Errorf("want a load of no entry, then one of %d", len(healthQuietNames))
					}
					return nil
				})
			},
		}
	}
	runHealthCases(t, cases)
}
