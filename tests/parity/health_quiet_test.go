// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The quiet rule set (M9 commit 5, D205 F1): six alerts, none of which ever goes above CLEAR. C runs the notifier for
// an entry whose status is WARNING or CRITICAL, and for a CLEAR that follows one (health_notifications.c:392-447: a
// status below CLEAR is "internal", and a CLEAR with no notification before it is the alert's first), and such an
// entry's row then carries the notifier's marks (EXEC_RUN in `flags`, `exec_run_timestamp`). These alerts get none, so
// what they leave in the alert log and in the health tables is the work of the log alone: the saves, their order, the
// queue of an unclaimed agent, the load at a restart.
//   - hq_tmpl: a template on the collected chart's context, with a class, a component, a type and a summary: CLEAR.
//   - hq_undef: CLEAR at 10, UNDEFINED at 70, CLEAR again at 10. Its warning divides by `$this - 70`: a division by
//     zero is an error in C's evaluator, whatever is done with its result (libnetdata/eval/eval-evaluate.c:152-177,
//     :301-332), a threshold that cannot be computed gives no status, and with no other threshold the alert is
//     UNDEFINED (health_event_loop.c:659-694).
//   - hq_delay: a notification delay of 6 s on the way up, so its first CLEAR waits 6 s to be processed: the entry is
//     saved when it is logged and again when the scan takes it (health_notifications.c:555-586).
//   - hq_var: no warning and no critical: UNDEFINED for ever, and a "variable" for the queue towards the Cloud, which
//     never moves it on (sqlite_aclk_alert.c:16-53, :155-156).
//   - hq_gone: on a chart that is defined obsolete and never collected: removed at HEALTH's first pass
//     (health_event_loop.c:486-523), so its last entry is REMOVED.
//   - hq_look: a lookup over 5 s, last in the file: its first CLEAR comes a pass or more after the others', once its
//     window has data (health_event_loop.c:173-187), so its entry is logged after the first store of the others'.
//
// Every alert is linked three times at HEALTH's first pass (healthPair.create): 18 entries. That pass also logs
// hq_gone's removal and the first status of the four alerts on a value (hq_tmpl, hq_undef, hq_delay, hq_var): 23.
// hq_look's first CLEAR is the 24th, hq_undef's UNDEFINED the 25th and its return to CLEAR the 26th.
const (
	healthQuietChart = "hq.values"
	healthQuietEmit  = `CHART hq.gone '' 'title' 'units' 'family' 'hq.gctx' line 1000 1 'obsolete' '' ''
DIMENSION a '' absolute 1 1
`
	healthQuietConf = `# the quiet alerts: none goes above CLEAR, so no notification runs
template: hq_tmpl
      on: hq.ctx
   class: Utilization
    type: System
component: Parity
    calc: $a
   every: 1s
    warn: $this > 500
   units: things
 summary: quiet on ${family}
    info: a template that stays CLEAR

   alarm: hq_undef
      on: hq.values
    calc: $a
   every: 1s
    warn: 1 / ($this - 70) > 1000
   units: things
    info: its warning cannot be computed at 70

   alarm: hq_delay
      on: hq.values
    calc: $a
   every: 1s
    warn: $this > 500
   delay: up 6s down 8s
   units: things
    info: its first status waits 6 seconds

   alarm: hq_var
      on: hq.values
    calc: $a * 2
   every: 1s
    info: no warning and no critical

   alarm: hq_gone
      on: hq.gone
    calc: $a
   every: 1s
    warn: $this > 500
   units: things
    info: on a chart that is obsolete

   alarm: hq_look
      on: hq.values
  lookup: max -5s unaligned of a
   every: 1s
    warn: $this > 500
   units: things
    info: the maximum of a over 5 seconds
`
)

// healthQuietPhases are the values of `a` the quiet cases play: hq_undef CLEAR, UNDEFINED, CLEAR; the others as they
// were.
var healthQuietPhases = []map[string]int64{{"a": 10}, {"a": 70}, {"a": 10}}

// healthQuietNames are the quiet alerts. `/api/v1/alarms?all` lists five of them: hq_gone's chart is obsolete, and C
// lists no alert of such a chart.
var healthQuietNames = []string{"hq_delay", "hq_gone", "hq_look", "hq_tmpl", "hq_undef", "hq_var"}

// healthQuietScenario is the fake plugin's scenario of a quiet case: hq.gone, obsolete and never collected, then the
// collected chart through the phases.
func healthQuietScenario() *plugin.Scenario {
	return healthScenario(healthQuietEmit, healthQuietChart, "hq.ctx", []string{"a"}, healthQuietPhases...)
}

// healthQuietWant are the statuses of the listed alerts while hq_undef has `undef`.
func healthQuietWant(undef string) map[string]string {
	return map[string]string{"hq_tmpl": "CLEAR", "hq_undef": undef, "hq_delay": "CLEAR", "hq_var": "UNDEFINED", "hq_look": "CLEAR"}
}

// healthQuietStatus is hq_undef's status at each phase, and healthQuietEntries the unique ids the host gave by the
// end of phase 0.
var healthQuietStatus = []string{"CLEAR", "UNDEFINED", "CLEAR"}

const healthQuietEntries = 24

// healthQuietMoved is how long a quiet case holds the first value, from the chart's creation: an unclaimed C queues
// each transition in alert_queue with a due second (a first CLEAR 10 s after it; sqlite_health.c:83-190) and HEALTH
// moves the due rows on at the end of a pass (sqlite_aclk_alert.c:728-784). A transition logged before its alert's row
// was moved rewrites the row, one logged after it makes a new one: which of the two it is shows in the count of rows
// moved (the access log's records), and with a transition a second or two from a due second it would be each side's
// own timing. The first statuses come one or two seconds after the chart's first second and hq_look's up to seven, so
// their rows are due by second 17: at 22 s every one was moved on both sides, with 5 s to spare.
const healthQuietMoved = 22 * time.Second

// healthPlayQuiet plays the quiet phases: 10 for healthQuietMoved, then 70, then 10 again. While they play only the
// oracle is judged: at each phase it must show the phase's statuses and the count of entries logged so far. The
// candidate then gets the bounded wait to show the same alerts (no verdict) before the next value comes: an alert's
// entries take their unique ids in the order they are logged, so a side that had not run hq_look yet when 70 arrived
// would number hq_undef's change before hq_look's first status.
func healthPlayQuiet(t *testing.T, h *healthPair) {
	t.Helper()
	h.create(t)
	all := func(i int) string { return h.get(i, "/api/v1/alarms?all") }
	for k, hold := range []time.Duration{0, healthQuietMoved, healthCalcHold} {
		if k > 0 {
			h.release(t, fmt.Sprintf("p%d", k), k, hold)
		}
		h.waitOracle(t, fmt.Sprintf("phase %d: /api/v1/alarms?all", k), func() (string, error) {
			v := all(0)
			return v, healthBoth(healthWant(healthQuietWant(healthQuietStatus[k])), healthLatest(healthQuietEntries+k))(v)
		})
		h.waitCandidate("/api/v1/alarms?all", all)
	}
}

// healthLogEntries is a guard on a view of `/api/v1/alarm_log`: the answer is 200 and holds `n` entries.
func healthLogEntries(n int) func(string) error {
	return func(view string) error {
		if err := healthHolds()(view); err != nil {
			return err
		}
		return healthTimes(`"unique_id":`, n)(view)
	}
}

// healthRowCount counts a table's rows in a side's dump of the health tables (healthNorm.healthDump).
func healthRowCount(rows []string, table string) int {
	n := 0
	for _, l := range rows {
		if strings.HasPrefix(l, "row "+table+" ") {
			n++
		}
	}
	return n
}

// healthRowsWant is a guard on a dump of the health tables: each table named has its count of rows.
func healthRowsWant(want map[string]int) func([]string) error {
	return func(rows []string) error {
		got := map[string]int{}
		for table := range want {
			got[table] = healthRowCount(rows, table)
		}
		for _, table := range healthTables {
			if w, ok := want[table]; ok && got[table] != w {
				return fmt.Errorf("the tables hold %v rows, want %v", got, want)
			}
		}
		return nil
	}
}

// The unclaimed queue's record in the access log (sqlite_aclk_alert.c:775-776): how many due rows a pass took from
// alert_queue, and how many of them it put into aclk_queue.
var healthQueueRecordRe = regexp.MustCompile(`msg="ACLK STA \[([^\]]*) \(N/A\)\]: Processed (\d+) entries, queued (\d+)"`)

// healthQueueMoves sums a side's queue records: which pass takes a due row follows each side's clock (rows due at
// two neighbouring seconds are taken by one pass or by two), so the records' count is each side's own; the rows taken
// and the rows moved on, in all, are the tables' and must agree. It returns `processed N, queued M in K records`
// without K, and K.
func healthQueueMoves(lines []string) (string, int) {
	processed, queued, records := 0, 0, 0
	for _, l := range lines {
		if m := healthQueueRecordRe.FindStringSubmatch(l); m != nil {
			var p, q int
			fmt.Sscan(m[2], &p)
			fmt.Sscan(m[3], &q)
			processed, queued, records = processed+p, queued+q, records+1
		}
	}
	return fmt.Sprintf("processed %d, queued %d", processed, queued), records
}

// queueMoves is side i's sum of the unclaimed queue's records (healthQueueMoves), from its access log.
func (h *healthPair) queueMoves(t *testing.T, i int) []string {
	t.Helper()
	moves, _ := healthQueueMoves(logLines(t, h.p.Each()[i].Daemon.Opts.RunDir, "access.log"))
	return []string{moves}
}

// The store job's record of the queued saves it stored (sqlite_metadata.c:2539-2544), a debug record of daemon.log.
var healthStoredRe = regexp.MustCompile(`msg="Stored and processed (\d+) alert transitions in `)

// healthStored sums a side's records of the queued saves the metadata thread stored. The entries logged when an alert
// is linked or unlinked are queued for that thread, the entries a pass logs itself are saved by HEALTH at once
// (health_log.c:68-76, rrdcalc.c:333, :360); C's record counts two for each queued save (its list holds a host and an
// entry for each, sqlite_metadata.c:2522, :2954-2971). Which store job takes a save follows each side's timing (the
// job HEALTH asks for at the end of a pass, or the 5 s timer's), so the records' count is each side's own; their sum
// is what the agent queued.
func healthStored(lines []string) string {
	stored := 0
	for _, l := range lines {
		if m := healthStoredRe.FindStringSubmatch(l); m != nil {
			var n int
			fmt.Sscan(m[1], &n)
			stored += n
		}
	}
	return fmt.Sprintf("stored %d alert transitions", stored)
}

// stored is side i's sum of the queued saves its store jobs took (healthStored), from its daemon.log; the case must
// run at debug level (healthLogsDebug).
func (h *healthPair) stored(t *testing.T, i int) []string {
	t.Helper()
	return []string{healthStored(logLines(t, h.p.Each()[i].Daemon.Opts.RunDir, "daemon.log"))}
}

// healthHashOf is the config hash the oracle's alert log names for an alert (its first entry's), or "".
func healthHashOf(entries []healthEntry, name string) string {
	i := slices.IndexFunc(entries, func(e healthEntry) bool { return e.Name == name })
	if i < 0 {
		return ""
	}
	return entries[i].Hash
}
