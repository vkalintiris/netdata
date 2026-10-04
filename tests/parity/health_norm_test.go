// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/notify"
)

// The health checks' normalizers (M9 commit 0, D183; plan evidence/2026-10-03-plan-m9-commit0.md §5.4). Two runs of C
// on the same inputs differ in what the wall clock and each run's directories give:
//   - an alert log's unique ids and alarm ids count from a wall-clock seed taken when the host's health starts
//     (database/sqlite/sqlite_health.c:863-871, database/rrd.h:91-93): rebased per side (`u+k`, `a+k`; 0 stays 0), so the
//     +1 steps and the links between entries (`updated_by_id`, `updates_id`) stay compared;
//   - a transition id is a random UUID (health/health_log.c:225): named after the rebased unique id of the entry the
//     host's alert log first showed it for (`t+k`). Only the alert log names an id (observe): the notifier's argument,
//     a health.log record and a spawn record are looked up, so an id the alert log never showed prints `t?`, another
//     entry's id that entry's name, and an entry that shows a second id `t!k`. A check that reads no alert log prints
//     a health.log's ids by the rank of their first appearance in the file (`t#k`, healthLogRanked);
//   - every time field is a wall-clock second: `T` when set, 0 kept (set or not is compared), a lookup's window as its
//     length (`db_before` as `T+<n>`). A notification's clock arguments print the name of the member of their own
//     alert log entry they equal (`when`, `duration`, `non_clear_duration`);
//   - each side's run directory and runtime directory: `{run}`, `{rt}`.
//
// The masks are wider than C's variation (a second or two side to side), so two bounds stand beside them
// (healthPair.near, healthPair.bases; D187 point 4): the times of events within healthBound side to side, and each
// side's id bases within it and not before the chart's first second.
//
// A side that serves no alert log (healthNorm.noLog; D198 F1: C's log is SQLite's, a later commit of the Rust agent)
// has no entry to name its unique ids' base: they print by its alarm ids' base. C seeds both with the same second on
// a host whose health starts on an empty table (sqlite_health.c:864-871), and healthBases holds the oracle, which
// has both, to that in every run.

// healthNorm is one side's normalizer state.
type healthNorm struct {
	run string
	// uBase and aBase are the side's id bases: an id prints as its distance from its base; uSet and aSet once known
	uBase, aBase int64
	uSet, aSet   bool
	// noLog: the side serves no alert log (healthPair.settle found none): until an entry names a unique base, its
	// unique ids print by the alarm ids' base
	noLog bool
	// log is the path of the host's alert log (localhost's; a child's is under `/host/<name>`)
	log string
	// What the host's alert log showed (observe): entries are its entries by unique id; tids the unique id of the
	// entry a transition id was first shown for; moved the unique id of an entry that showed the id after another one
	entries     map[int64]healthEntry
	tids, moved map[string]int64
}

func newHealthNorm(d *daemon.Daemon, prefix string) *healthNorm {
	return &healthNorm{run: d.Opts.RunDir, log: prefix + "/api/v1/alarm_log", entries: map[int64]healthEntry{},
		tids: map[string]int64{}, moved: map[string]int64{}}
}

var (
	healthUniqueRe = regexp.MustCompile(`"(unique_id|latest_alarm_log_unique_id|updated_by_id|updates_id)":\s*(\d+)`)
	healthAlarmRe  = regexp.MustCompile(`"(alarm_id|id)":\s*(\d+)`)
	// an alert log entry's transition id (sqlite_health.c:1157-1161: after its unique id, alarm id, event id and
	// config hash)
	healthTidRe = regexp.MustCompile(`"transition_id":\s*"([^"]*)"`)
	// the time members of /api/v1/alarms and /api/v1/alarm_log (health/health_json.c:53-103, sqlite_health.c:1160-1200);
	// last_repeat is a quoted number in the first and a number in the second
	healthTimeRe = regexp.MustCompile(`"(now|when|exec_run|delay_up_to_timestamp|last_repeat|last_status_change|last_updated|next_update|duration|non_clear_duration)":(\s*)("?)(\d+)("?)`)
	healthDBRe   = regexp.MustCompile(`"db_after":(\s*)(\d+),(\s*)"db_before":(\s*)(\d+)`)
)

// observe takes what a body shows of the side's ids: a base is the smallest id seen, less one (the first id is the
// seed plus one; healthPair.settle waits for the alert log's first entries, so the bases are the seeds before
// anything is compared). An alert log (a list of entries) also gives each entry's members and its transition id:
// the first id an entry shows is the entry's, and the first entry that shows an id owns it.
func (n *healthNorm) observe(body string) {
	for _, m := range healthUniqueRe.FindAllStringSubmatch(body, -1) {
		if v, _ := strconv.ParseInt(m[2], 10, 64); v > 0 && m[1] == "unique_id" && (!n.uSet || v-1 < n.uBase) {
			n.uBase, n.uSet = v-1, true
		}
	}
	for _, m := range healthAlarmRe.FindAllStringSubmatch(body, -1) {
		if v, _ := strconv.ParseInt(m[2], 10, 64); v > 0 && (!n.aSet || v-1 < n.aBase) {
			n.aBase, n.aSet = v-1, true
		}
	}
	var entries []healthEntry
	if json.Unmarshal([]byte(body), &entries) != nil {
		return
	}
	for _, e := range entries {
		if e.UniqueID == 0 {
			continue
		}
		_, owned := n.tids[e.Tid]
		if first, seen := n.entries[e.UniqueID]; seen && first.Tid != "" {
			// the entry keeps the id it first showed
			if e.Tid != first.Tid && e.Tid != "" && !owned {
				n.moved[e.Tid] = e.UniqueID
			}
			e.Tid = first.Tid
		} else if e.Tid != "" && !owned {
			n.tids[e.Tid] = e.UniqueID
		}
		n.entries[e.UniqueID] = e
	}
}

// unique and alarm print an id as its distance from the side's base; a side without an alert log (noLog) has one
// base for both.
func (n *healthNorm) unique(v int64) string {
	if n.noLog && !n.uSet {
		return healthRebase("u", v, n.aBase, n.aSet)
	}
	return healthRebase("u", v, n.uBase, n.uSet)
}
func (n *healthNorm) alarm(v int64) string { return healthRebase("a", v, n.aBase, n.aSet) }

// healthLowestAlarm is the lowest alarm id a body shows (an /api/v1/alarms answer's `id` members).
func healthLowestAlarm(body string) (int64, bool) {
	lowest, ok := int64(0), false
	for _, m := range healthAlarmRe.FindAllStringSubmatch(body, -1) {
		if v, _ := strconv.ParseInt(m[2], 10, 64); v > 0 && (!ok || v < lowest) {
			lowest, ok = v, true
		}
	}
	return lowest, ok
}

// anchor sets the alarm ids' base of a side without an alert log: `lowest`, the lowest alarm id its /api/v1/alarms?all
// lists, is the `k`th id after the base, as the oracle's lowest listed id is after the oracle's base. Nothing but the
// alert log shows the ids an alert of a never-collected chart took, and such a chart created first takes the seed's
// first ids: the lowest listed id is then not the base's next. A lower id seen later still lowers the base (observe).
func (n *healthNorm) anchor(lowest, k int64) {
	n.aBase, n.aSet = lowest-k, true
}

func healthRebase(prefix string, v, base int64, set bool) string {
	switch {
	case v == 0:
		return "0"
	case !set:
		return prefix + "?"
	}
	return fmt.Sprintf("%s%+d", prefix, v-base)
}

// tid names a transition id after the unique id of the entry the alert log first showed it for (`t+k`); the id an
// entry showed after another one prints `t!k`, one no entry showed `t?` (none and the nil id as they are).
func (n *healthNorm) tid(uuid string) string {
	if v, ok := n.tids[uuid]; ok {
		return "t" + strings.TrimPrefix(n.unique(v), "u")
	}
	if v, ok := n.moved[uuid]; ok {
		return "t!" + strings.TrimLeft(n.unique(v), "u+")
	}
	if uuid == "" || uuid == "00000000-0000-0000-0000-000000000000" {
		return uuid
	}
	return "t?"
}

// paths replaces the side's run directory and runtime directory.
func (n *healthNorm) paths(s string) string {
	return shortRunRe.ReplaceAllString(strings.ReplaceAll(s, n.run, "{run}"), "{rt}")
}

// healthClock masks a body's time members: `T` when set, 0 kept; a lookup's window as its length.
func healthClock(body string) string {
	body = healthDBRe.ReplaceAllStringFunc(body, func(m string) string {
		g := healthDBRe.FindStringSubmatch(m)
		after, _ := strconv.ParseInt(g[2], 10, 64)
		before, _ := strconv.ParseInt(g[5], 10, 64)
		if after == 0 || before == 0 {
			return m
		}
		return fmt.Sprintf(`"db_after":%sT,%s"db_before":%sT%+d`, g[1], g[3], g[4], before-after)
	})
	return healthTimeRe.ReplaceAllStringFunc(body, func(m string) string {
		g := healthTimeRe.FindStringSubmatch(m)
		if g[4] == "0" {
			return m
		}
		return `"` + g[1] + `":` + g[2] + g[3] + "T" + g[5]
	})
}

// json renders a hand-built v1 body (alarms, alarm_log): its bytes with the ids rebased, the transition ids named,
// the clock masked and the side's directories replaced; everything else, the tabs and the quoting too, is compared.
func (n *healthNorm) json(body string) string {
	n.observe(body)
	body = healthUniqueRe.ReplaceAllStringFunc(body, func(m string) string {
		g := healthUniqueRe.FindStringSubmatch(m)
		v, _ := strconv.ParseInt(g[2], 10, 64)
		return strings.TrimSuffix(m, g[2]) + n.unique(v)
	})
	body = healthAlarmRe.ReplaceAllStringFunc(body, func(m string) string {
		g := healthAlarmRe.FindStringSubmatch(m)
		v, _ := strconv.ParseInt(g[2], 10, 64)
		return strings.TrimSuffix(m, g[2]) + n.alarm(v)
	})
	body = healthTidRe.ReplaceAllStringFunc(body, func(m string) string {
		g := healthTidRe.FindStringSubmatch(m)
		return strings.TrimSuffix(m, `"`+g[1]+`"`) + `"` + n.tid(g[1]) + `"`
	})
	return n.paths(healthClock(body))
}

// Positions in a notification's argv (argv[0] is the script; health/health_notifications.c:107-150, :471-508).
const (
	healthArgHost     = 2
	healthArgUnique   = 3
	healthArgAlarm    = 4
	healthArgWhen     = 6
	healthArgDuration = 14
	healthArgNonClear = 15
	healthArgGUID     = 28
	healthArgTid      = 29
)

// call renders one notifier call as two agents must agree on it: its arguments (args), the environment (the side's
// directories replaced, then maskEnv: each agent's invocation id), the directory, the parent's name, the rule it
// matched and how it ended. The call's number is its position in the transcript.
func (n *healthNorm) call(c notify.Call) []string {
	out := []string{}
	for i, a := range n.args(c.Argv) {
		out = append(out, fmt.Sprintf("argv[%d]=%s", i, a))
	}
	var env []string
	for _, e := range c.Env {
		env = append(env, n.paths(e))
	}
	for _, e := range maskEnv(env) {
		out = append(out, "env "+e)
	}
	// no end record: the call was killed, or still runs
	end := c.End
	if end == "" {
		end = "none"
	}
	return append(out, "cwd "+n.paths(c.Cwd), "parent "+c.ParentComm, fmt.Sprintf("rule %d", c.Rule), "end "+end)
}

// args renders a notification's arguments (the first is the script): the ids rebased, the transition id named
// (tid), the side's directories replaced, and the three clock arguments (the transition's time and its two durations,
// health_notifications.c:107-150) by the name of the member they equal in the alert log's entry of the call's unique
// id: `when`, `duration`, `non_clear_duration`. 0, and a value that is not the entry's, print as they are.
func (n *healthNorm) args(argv []string) []string {
	var e healthEntry
	known := false
	if len(argv) > healthArgUnique {
		if v, err := strconv.ParseInt(argv[healthArgUnique], 10, 64); err == nil {
			e, known = n.entries[v]
		}
	}
	own := func(a string, member int64, name string) string {
		if known && a != "0" && a == strconv.FormatInt(member, 10) {
			return name
		}
		return a
	}
	out := make([]string, len(argv))
	for i, a := range argv {
		switch i {
		case healthArgUnique:
			if v, err := strconv.ParseInt(a, 10, 64); err == nil {
				a = n.unique(v)
			}
		case healthArgAlarm:
			if v, err := strconv.ParseInt(a, 10, 64); err == nil {
				a = n.alarm(v)
			}
		case healthArgWhen:
			a = own(a, e.When, "when")
		case healthArgDuration:
			a = own(a, e.Duration, "duration")
		case healthArgNonClear:
			a = own(a, e.NonClear, "non_clear_duration")
		case healthArgTid:
			a = n.tid(a)
		}
		out[i] = n.paths(a)
	}
	return out
}

// healthCommandRe is a notification's command in a record (the spawn server's, about a call that failed or was
// killed): `exec '<script>' '<a1>' … '<a33>'`, up to the quote that closes the logged `/bin/sh -c "…"`.
var healthCommandRe = regexp.MustCompile(`exec '(.*)'\\"`)

// command renders the notification's command inside a record with its arguments as args does.
func (n *healthNorm) command(record string) string {
	return healthCommandRe.ReplaceAllStringFunc(record, func(m string) string {
		words := n.args(strings.Split(healthCommandRe.FindStringSubmatch(m)[1], "' '"))
		return "exec '" + strings.Join(words, "' '") + `'\"`
	})
}

// calls renders a side's notifier transcript. The record files are read first, then the host's alert log: C saves an
// entry before it spawns its notification (health_event_loop.c:756, health_notifications.c:516), so every call read
// has its entry. The calls are in the order of their entries' unique ids (argument 3), a repeat after the call before
// it: C spawns a pass's notifications newest entry first and waits afterwards (health_notifications.c:516-519,
// :562), so the order the stubs started in is each side's pass phase.
func (n *healthNorm) calls(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	calls, err := notify.Calls(d.Opts.RunDir)
	if err != nil {
		t.Fatal(err)
	}
	if r := healthGet(d, n.log); r.Status == http.StatusOK {
		n.observe(string(r.Body))
	}
	unique := func(c notify.Call) int64 {
		if len(c.Argv) <= healthArgUnique {
			return 0
		}
		v, _ := strconv.ParseInt(c.Argv[healthArgUnique], 10, 64)
		return v
	}
	slices.SortStableFunc(calls, func(a, b notify.Call) int {
		switch ua, ub := unique(a), unique(b); {
		case ua < ub:
			return -1
		case ua > ub:
			return 1
		}
		return 0
	})
	var out []string
	for i, c := range calls {
		for _, l := range n.call(c) {
			out = append(out, fmt.Sprintf("call %d: %s", i+1, l))
		}
	}
	return out
}

// A health.log record's fields that follow the wall clock or a run's ids (libnetdata/log/nd_log-internals.c:609-710).
var (
	healthLogUniqueRe = regexp.MustCompile(` alert_unique_id=(\d+)`)
	healthLogAlarmRe  = regexp.MustCompile(` alert_id=(\d+)`)
	healthLogTidRe    = regexp.MustCompile(` alert_transition_id=([0-9a-f]{32})\b`)
	healthLogTimeRe   = regexp.MustCompile(` (alert_duration|alert_notification_timestamp)=("[^"]*"|\S+)`)
)

// healthDashed is a UUID of 32 hex digits (a log field) in the dashed form the API and the notifier get.
func healthDashed(u string) string {
	if len(u) != 32 {
		return u
	}
	return u[:8] + "-" + u[8:12] + "-" + u[12:16] + "-" + u[16:20] + "-" + u[20:]
}

// healthLog renders a side's health.log in file order: each record with its time, thread id, unique id, alarm id,
// transition id (looked up: tid), duration and notification time (`T` when set, 0 kept) normalized and the side's
// directories replaced.
func (n *healthNorm) healthLog(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	return n.healthLogWith(t, d, func(dashed string) string { return n.tid(dashed) })
}

// healthLogRanked is healthLog for a check that reads no alert log (D198 F1): a transition id prints as the rank of
// its first appearance in the file (`t#k`), so two records that share an id, or a record that repeats an earlier
// one's, still show; the nil id and an empty one stay as they are.
func (n *healthNorm) healthLogRanked(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	ranks := map[string]int{}
	return n.healthLogWith(t, d, func(dashed string) string {
		if dashed == "" || dashed == "00000000-0000-0000-0000-000000000000" {
			return dashed
		}
		if _, seen := ranks[dashed]; !seen {
			ranks[dashed] = len(ranks) + 1
		}
		return fmt.Sprintf("t#%d", ranks[dashed])
	})
}

// healthLogWith renders a side's health.log with `tid` naming each record's transition id (given dashed).
func (n *healthNorm) healthLogWith(t *testing.T, d *daemon.Daemon, tid func(dashed string) string) []string {
	t.Helper()
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "health.log") {
		l = healthLogUniqueRe.ReplaceAllStringFunc(l, func(m string) string {
			v, _ := strconv.ParseInt(strings.TrimPrefix(m, " alert_unique_id="), 10, 64)
			return " alert_unique_id=" + n.unique(v)
		})
		l = healthLogAlarmRe.ReplaceAllStringFunc(l, func(m string) string {
			v, _ := strconv.ParseInt(strings.TrimPrefix(m, " alert_id="), 10, 64)
			return " alert_id=" + n.alarm(v)
		})
		l = healthLogTidRe.ReplaceAllStringFunc(l, func(m string) string {
			return " alert_transition_id=" + tid(healthDashed(strings.TrimPrefix(m, " alert_transition_id=")))
		})
		l = healthLogTimeRe.ReplaceAllStringFunc(l, func(m string) string {
			g := healthLogTimeRe.FindStringSubmatch(m)
			if g[2] == "0" {
				return m
			}
			return " " + g[1] + "=T"
		})
		out = append(out, n.paths(normalizeLog(l, d.Opts.RunDir, "")))
	}
	return out
}

// The health tables of the metadata database (database/sqlite/sqlite_metadata.c:55-105).
var healthTables = []string{"alert_hash", "health_log", "health_log_detail", "alert_queue", "alert_version", "aclk_queue",
	"alert_hash_cloud"}

var (
	// integer ids and clocks in a metadata-dump row (`name=value`)
	healthSQLUniqueRe = regexp.MustCompile(` (unique_id|updated_by_id|updates_id)=(\d+)`)
	healthSQLAlarmRe  = regexp.MustCompile(` (alarm_id)=(\d+)`)
	healthSQLTimeRe   = regexp.MustCompile(` (when_key|duration|non_clear_duration|exec_run_timestamp|delay_up_to_timestamp|last_repeat|date_updated)=(\d+)`)
)

// healthDump prints a side's health tables with metadata-dump (the transition ids aliased by first appearance, the
// global ids masked: µs clocks), then rebases the integer ids, masks the clock columns (0 and NULL kept) and replaces
// the side's directories.
func (n *healthNorm) healthDump(t *testing.T, d *daemon.Daemon, more ...string) []string {
	t.Helper()
	args := []string{"--mask", "health_log_detail.global_id", "--alias", "health_log_detail.transition_id",
		"--alias", "health_log.last_transition_id"}
	for _, table := range healthTables {
		args = append(args, "--table", table)
	}
	dump := dumpDB(t, filepath.Join(d.Opts.RunDir, "cache", "netdata-meta.db"), append(args, more...)...)
	var out []string
	for _, l := range strings.Split(strings.TrimRight(dump, "\n"), "\n") {
		if !strings.HasPrefix(l, "row ") && !strings.HasPrefix(l, "missing ") {
			continue
		}
		l = healthSQLUniqueRe.ReplaceAllStringFunc(l, func(m string) string {
			g := healthSQLUniqueRe.FindStringSubmatch(m)
			v, _ := strconv.ParseInt(g[2], 10, 64)
			return " " + g[1] + "=" + n.unique(v)
		})
		l = healthSQLAlarmRe.ReplaceAllStringFunc(l, func(m string) string {
			g := healthSQLAlarmRe.FindStringSubmatch(m)
			v, _ := strconv.ParseInt(g[2], 10, 64)
			return " " + g[1] + "=" + n.alarm(v)
		})
		l = healthSQLTimeRe.ReplaceAllStringFunc(l, func(m string) string {
			g := healthSQLTimeRe.FindStringSubmatch(m)
			if g[2] == "0" {
				return m
			}
			return " " + g[1] + "=T"
		})
		out = append(out, n.paths(l))
	}
	return out
}

// The normalizers change what follows the clock, the seeds and the side's directories, and nothing else: ids print by
// their distance from the side's base (0 kept), a set time as T (0 kept), a lookup's window by its length. A
// transition id is named only by the alert log: a notifier's argument, a health.log record or a spawn record that
// carries another id does not print as the entry's. The bounds beside the masks fail clocks more than healthBound
// apart: the events' times, and the seconds the variables' endpoints' masks replaced. A side without an alert log
// prints its unique ids by its alarm ids' base and a health.log's transition ids by their rank; the bases' bound
// holds the oracle to one base for both. The alert members of a v2 data answer are taken as they are.
func TestHealthNorm(t *testing.T) {
	run := filepath.Join(t.TempDir(), "oracle")
	blank := func() *healthNorm {
		return &healthNorm{run: run, log: "/api/v1/alarm_log", entries: map[int64]healthEntry{}, tids: map[string]int64{},
			moved: map[string]int64{}}
	}
	n := blank()
	tid, other := "11111111-2222-4333-8444-555555555555", "99999999-2222-4333-8444-555555555555"
	entry := func(unique int, tid string) string {
		return fmt.Sprintf(`{"unique_id":%d,"alarm_id":501,"alarm_event_id":2,"config_hash_id":"aaaaaaaa-0000-4000-8000-000000000001",`+
			`"transition_id":"%s","name":"a","exec":"%s/notify/stub","when":1790000000,"duration":0,`+
			`"non_clear_duration":7,"delay":0,"delay_up_to_timestamp":1790000000,"updated_by_id":0,"updates_id":1000,`+
			`"last_repeat":0,"value":70}`, unique, tid, run)
	}
	rendered := func(unique, tid string) string {
		return `{"unique_id":` + unique + `,"alarm_id":a+1,"alarm_event_id":2,"config_hash_id":"aaaaaaaa-0000-4000-8000-000000000001",` +
			`"transition_id":"` + tid + `","name":"a","exec":"{run}/notify/stub","when":T,"duration":0,` +
			`"non_clear_duration":T,"delay":0,"delay_up_to_timestamp":T,"updated_by_id":0,"updates_id":u+1,` +
			`"last_repeat":0,"value":70}`
	}
	// the base is the lowest unique id seen, less one: an older entry (the link's, stored later) moves it
	n.observe(`{"unique_id":1000,"alarm_id":501}`)
	if got, want := n.json("["+entry(1001, tid)+"]"), "["+rendered("u+2", "t+2")+"]"; got != want {
		t.Errorf("alarm_log:\n got %s\nwant %s", got, want)
	}
	alarms := "\t\"latest_alarm_log_unique_id\": 1001,\n\t\"now\": 1790000001,\n\t\t\t\"id\": 502,\n\t\t\t\"delay_up_duration\": 3,\n" +
		"\t\t\t\"last_repeat\": \"0\",\n\t\t\t\"last_updated\": 1790000001,\n\t\t\t\"update_every\": 1,\n" +
		"\t\t\t\"db_after\": 1789999997,\n\t\t\t\"db_before\": 1790000001,\n\t\t\t\"value\":70\n"
	wantAlarms := "\t\"latest_alarm_log_unique_id\": u+2,\n\t\"now\": T,\n\t\t\t\"id\": a+2,\n\t\t\t\"delay_up_duration\": 3,\n" +
		"\t\t\t\"last_repeat\": \"0\",\n\t\t\t\"last_updated\": T,\n\t\t\t\"update_every\": 1,\n" +
		"\t\t\t\"db_after\": T,\n\t\t\t\"db_before\": T+4,\n\t\t\t\"value\":70\n"
	if got := n.json(alarms); got != wantAlarms {
		t.Errorf("alarms:\n got %q\nwant %q", got, wantAlarms)
	}

	// a notification's arguments: the clock arguments by the name of the entry's member they equal, the transition id
	// by the entry the alert log showed it for
	argv := func(set map[int]string) []string {
		out := make([]string, 34)
		for i := range out {
			out[i] = fmt.Sprintf("w%d", i)
		}
		for i, v := range map[int]string{healthArgUnique: "1001", healthArgAlarm: "501", 5: "2", healthArgWhen: "1790000000",
			13: "line=2,file=" + run + "/etc/health.d/parity.conf", healthArgDuration: "0", healthArgNonClear: "7",
			24: "b=1790000003,a=1790000002", healthArgTid: tid, 30: "1790000000"} {
			out[i] = v
		}
		for i, v := range set {
			out[i] = v
		}
		return out
	}
	for name, c := range map[string]struct {
		set  map[int]string
		want map[int]string
	}{
		"the entry's own": {nil, map[int]string{healthArgUnique: "u+2", healthArgAlarm: "a+1", 5: "2", healthArgWhen: "when",
			13: "line=2,file={run}/etc/health.d/parity.conf", healthArgDuration: "0", healthArgNonClear: "non_clear_duration",
			24: "b=1790000003,a=1790000002", healthArgTid: "t+2", 30: "1790000000"}},
		// what a self-named id hid (R75 F1): any text printed as the call's own entry
		"an id no entry showed":  {map[int]string{healthArgTid: other}, map[int]string{healthArgTid: "t?"}},
		"no id":                  {map[int]string{healthArgTid: ""}, map[int]string{healthArgTid: ""}},
		"not an id":              {map[int]string{healthArgTid: "garbage"}, map[int]string{healthArgTid: "t?"}},
		"the id without dashes":  {map[int]string{healthArgTid: strings.ReplaceAll(tid, "-", "")}, map[int]string{healthArgTid: "t?"}},
		"another time":           {map[int]string{healthArgWhen: "1790000003"}, map[int]string{healthArgWhen: "1790000003"}},
		"another duration":       {map[int]string{healthArgDuration: "5", healthArgNonClear: "0"}, map[int]string{healthArgDuration: "5", healthArgNonClear: "0"}},
		"an entry nobody showed": {map[int]string{healthArgUnique: "1005"}, map[int]string{healthArgUnique: "u+6", healthArgWhen: "1790000000", healthArgNonClear: "7", healthArgTid: "t+2"}},
	} {
		got := n.args(argv(c.set))
		for i, want := range c.want {
			if got[i] != want {
				t.Errorf("%s: argument %d prints %q, want %q", name, i, got[i], want)
			}
		}
	}
	record := `msg="SPAWN SERVER: child with pid P exited with exit code 3: /bin/sh -c \"exec '<RUN>/notify/stub' 'root' 'h' '1001' '501' '2' '1790000000' 'a'\""`
	wantRecord := `msg="SPAWN SERVER: child with pid P exited with exit code 3: /bin/sh -c \"exec '<RUN>/notify/stub' 'root' 'h' 'u+2' 'a+1' '2' 'when' 'a'\""`
	if got := n.command(record); got != wantRecord {
		t.Errorf("command:\n got %s\nwant %s", got, wantRecord)
	}

	// an id belongs to the entry that showed it first, and an entry keeps the id it showed first
	if got, want := n.json("["+entry(1002, tid)+"]"), "["+rendered("u+3", "t+2")+"]"; got != want {
		t.Errorf("another entry with the first one's id:\n got %s\nwant %s", got, want)
	}
	if got, want := n.json("["+entry(1001, other)+"]"), "["+rendered("u+2", "t!2")+"]"; got != want {
		t.Errorf("an entry with another id at a later read:\n got %s\nwant %s", got, want)
	}
	if got := n.tid(tid); got != "t+2" {
		t.Errorf("the first id prints %q after the entry showed another", got)
	}
	// a side that showed no id yet prints `?`, never a number another side could match by chance
	if got := blank().unique(1001); got != "u?" {
		t.Errorf("an id without a base prints %q", got)
	}

	// health.log: the transition id is looked up, a duration of 0 stays 0
	d := &daemon.Daemon{Opts: daemon.Options{RunDir: run}}
	if err := os.MkdirAll(filepath.Join(run, "log"), 0o755); err != nil {
		t.Fatal(err)
	}
	record = "time=2026-10-04T00:00:00.000Z comm=netdata source=health level=warning tid=7 thread=HEALTH alert_id=501 alert_unique_id=1001 " +
		"alert_transition_id=%s alert_duration=%s alert=a msg=x\n"
	lines := fmt.Sprintf(record, strings.ReplaceAll(tid, "-", ""), "0") + fmt.Sprintf(record, strings.Repeat("9", 32), "12")
	if err := os.WriteFile(filepath.Join(run, "log", "health.log"), []byte(lines), 0o644); err != nil {
		t.Fatal(err)
	}
	wantLog := []string{
		"time=T comm=netdata source=health level=warning tid=N thread=HEALTH alert_id=a+1 alert_unique_id=u+2 alert_transition_id=t+2 alert_duration=0 alert=a msg=x",
		"time=T comm=netdata source=health level=warning tid=N thread=HEALTH alert_id=a+1 alert_unique_id=u+2 alert_transition_id=t? alert_duration=T alert=a msg=x",
	}
	if got := n.healthLog(t, d); !slices.Equal(got, wantLog) {
		t.Errorf("health.log:\n got %q\nwant %q", got, wantLog)
	}

	// a transcript: the record files, then the alert log (the ids' names), in the order of the unique ids
	log := "[" + entry(1002, other) + "," + entry(1001, tid) + "]"
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { fmt.Fprint(w, log) }))
	defer srv.Close()
	d.BaseURL = srv.URL
	if err := os.MkdirAll(notify.Dir(run), 0o755); err != nil {
		t.Fatal(err)
	}
	// started newest first, as C spawns a pass's notifications
	for k, c := range []notify.Call{{Seq: 1, Argv: argv(map[int]string{healthArgUnique: "1002", healthArgTid: other})}, {Seq: 2, Argv: argv(nil)}} {
		b, _ := json.Marshal(c)
		if err := os.WriteFile(filepath.Join(notify.Dir(run), fmt.Sprintf("call-%05d.json", k+1)), b, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	fresh := blank()
	fresh.observe(`{"unique_id":1000,"alarm_id":501}`)
	var ids []string
	for _, l := range fresh.calls(t, d) {
		if strings.Contains(l, fmt.Sprintf(": argv[%d]=", healthArgUnique)) || strings.Contains(l, fmt.Sprintf(": argv[%d]=", healthArgTid)) {
			ids = append(ids, l)
		}
	}
	if want := []string{"call 1: argv[3]=u+2", "call 1: argv[29]=t+2", "call 2: argv[3]=u+3", "call 2: argv[29]=t+3"}; !slices.Equal(ids, want) {
		t.Errorf("a transcript's ids: %q, want %q", ids, want)
	}

	// the bound beside the clock masks: the times of events side to side, not the times of the read
	body := func(when, updated int) string {
		return fmt.Sprintf(`[{"when":%d,"last_updated":%d,"duration":4,"delay_up_duration":3,"exec_run":"%d"}]`, when, updated, when+1)
	}
	for name, c := range map[string]struct {
		candidate string
		want      string
	}{
		"the same":            {body(1790000000, 1790000000), ""},
		"two seconds later":   {body(1790000002, 1790000009), ""},
		"three seconds later": {body(1790000003, 1790000000), "event time 1: `when` is 1790000000 on the oracle"},
		"earlier":             {body(1789999997, 1790000000), "event time 1: `when`"},
		"one member less":     {`[{"when":1790000000}]`, "3 event times on the oracle, 1 on the candidate"},
	} {
		err := healthNear(body(1790000000, 1790000000), c.candidate)
		if (err == nil) != (c.want == "") || (err != nil && !strings.Contains(err.Error(), c.want)) {
			t.Errorf("near, %s: %v, want %q", name, err, c.want)
		}
	}

	// the variables' endpoints: the read's second and a set collection second print T (0, never collected, is kept),
	// and the masked seconds come back for the bound
	vars := "{\n\"after\":1790000000,\n\"before\":1790000001,\n\"now\":1790000001,\n\"a_last_collected_t\":1789999999,\n" +
		"\"p.q_last_collected_t\":0,\n\"last_collected_t\":1789999998,\n\"a_raw\":10\n}"
	wantVars := "{\n\"after\":T,\n\"before\":T+1,\n\"now\":T+1,\n\"a_last_collected_t\":T,\n" +
		"\"p.q_last_collected_t\":0,\n\"last_collected_t\":T,\n\"a_raw\":10\n}"
	if got, clocks := healthVars(vars); got != wantVars || !slices.Equal(clocks, []int64{1790000000, 1789999999, 1789999998}) {
		t.Errorf("alarm_variables:\n got %q %v\nwant %q", got, clocks, wantVars)
	}
	trace := func(variable, value string) string {
		return "{\n\"variable\":\"" + variable + "\",\n\"instance\":\"c.a\",\n\"context\":\"c\",\n\"found\":true,\n\"value\":" + value +
			",\n\"source\":{\n\"candidates\":1\n}\n}"
	}
	for name, c := range map[string]struct {
		variable, value, want string
		clocks                []int64
	}{
		"the wall clock":                {"now", "1790000001", "T", []int64{1790000001}},
		"the chart's last collection":   {"last_collected_t", "1790000000", "T", []int64{1790000000}},
		"a dimension's":                 {"c.a.a_last_collected_t", "1790000000", "T", []int64{1790000000}},
		"never collected":               {"a_last_collected_t", "0", "0", nil},
		"a value":                       {"a", "1790000001", "1790000001", nil},
		"a name that only ends like it": {"is_now", "1790000001", "1790000001", nil},
		"no value":                      {"now", "null", "null", nil},
	} {
		if got, clocks := healthTrace(trace(c.variable, c.value)); got != trace(c.variable, c.want) || !slices.Equal(clocks, c.clocks) {
			t.Errorf("a trace, %s: %q %v, want the value %s and %v", name, got, clocks, c.want, c.clocks)
		}
	}
	for name, c := range map[string]struct {
		oracle, candidate []int64
		want              string
	}{
		"the same":            {[]int64{1790000000, 1790000001}, []int64{1790000000, 1790000001}, ""},
		"two seconds later":   {[]int64{1790000000}, []int64{1790000002}, ""},
		"three seconds later": {[]int64{1790000000}, []int64{1790000003}, "masked second 1 is 1790000000 on the oracle"},
		"not a clock":         {[]int64{1790000000}, []int64{1}, "masked second 1"},
		"one less":            {[]int64{1790000000, 1790000001}, []int64{1790000000}, "2 masked seconds on the oracle, 1 on the candidate"},
		"none":                {nil, nil, ""},
	} {
		err := healthClocksNear(c.oracle, c.candidate)
		if (err == nil) != (c.want == "") || (err != nil && !strings.Contains(err.Error(), c.want)) {
			t.Errorf("the masked seconds, %s: %v, want %q", name, err, c.want)
		}
	}

	// a side that serves no alert log (noLog): its unique ids print by its alarm ids' base, which is set from the lowest
	// alarm id its /api/v1/alarms?all lists (anchor); a side with a log never borrows that base
	if lowest, ok := healthLowestAlarm("\t\t\t\"id\": 508,\n\t\t\t\"id\": 507,\n\t\"latest_alarm_log_unique_id\": 3,\n\t\t\"alarm_event_id\": 2,\n"); !ok || lowest != 507 {
		t.Errorf("the lowest alarm id listed: %d %v, want 507", lowest, ok)
	}
	if lowest, ok := healthLowestAlarm("\t\"latest_alarm_log_unique_id\": 0,\n\t\"alarms\": {\n\n\t}\n"); ok {
		t.Errorf("an answer that lists no alert shows the alarm id %d", lowest)
	}
	listed := "\t\"latest_alarm_log_unique_id\": 520,\n\t\t\t\"id\": 507,\n\t\t\t\"id\": 508,\n"
	for name, c := range map[string]struct {
		noLog  bool
		anchor [2]int64
		entry  string
		want   string
	}{
		// the oracle lists its first alarm id first: the candidate's lowest is its base's next
		"no log, the first id listed": {true, [2]int64{507, 1}, "", "\t\"latest_alarm_log_unique_id\": u+14,\n\t\t\t\"id\": a+1,\n\t\t\t\"id\": a+2,\n"},
		// seven alerts of a chart that was never collected took the ids before the first one listed
		"no log, the eighth id listed": {true, [2]int64{507, 8}, "", "\t\"latest_alarm_log_unique_id\": u+21,\n\t\t\t\"id\": a+8,\n\t\t\t\"id\": a+9,\n"},
		"no log, no alarm id seen":     {true, [2]int64{}, "", "\t\"latest_alarm_log_unique_id\": u+14,\n\t\t\t\"id\": a+1,\n\t\t\t\"id\": a+2,\n"},
		// an entry's unique id, once one shows, is the unique ids' base again
		"no log, then an entry": {true, [2]int64{507, 1}, `{"unique_id":511,"alarm_id":507}`, "\t\"latest_alarm_log_unique_id\": u+10,\n\t\t\t\"id\": a+1,\n\t\t\t\"id\": a+2,\n"},
		"a log, no entry yet":   {false, [2]int64{507, 1}, "", "\t\"latest_alarm_log_unique_id\": u?,\n\t\t\t\"id\": a+1,\n\t\t\t\"id\": a+2,\n"},
	} {
		side := blank()
		side.noLog = c.noLog
		if c.anchor[0] != 0 {
			side.anchor(c.anchor[0], c.anchor[1])
		}
		if c.entry != "" {
			side.observe(c.entry)
		}
		if got := side.json(listed); got != c.want {
			t.Errorf("%s:\n got %q\nwant %q", name, got, c.want)
		}
	}
	if got := func() string { side := blank(); side.noLog = true; return side.unique(520) }(); got != "u?" {
		t.Errorf("without a log and before any alarm id, a unique id prints %q", got)
	}

	// the bound on the bases, and the ground of the fallback: the oracle's two bases are one
	bases := func(u, a int64, uSet, aSet, noLog bool) *healthNorm {
		side := blank()
		side.uBase, side.aBase, side.uSet, side.aSet, side.noLog = u, a, uSet, aSet, noLog
		return side
	}
	for name, c := range map[string]struct {
		oracle, candidate *healthNorm
		want              string
	}{
		"the same second":              {bases(1001, 1001, true, true, false), bases(1001, 1001, true, true, false), ""},
		"two seconds later":            {bases(1001, 1001, true, true, false), bases(1003, 1003, true, true, false), ""},
		"three seconds later":          {bases(1001, 1001, true, true, false), bases(1004, 1001, true, true, false), "the unique ids' bases: oracle 1001, candidate 1004"},
		"before the chart":             {bases(1001, 1001, true, true, false), bases(1001, 999, true, true, false), "the alarm ids' bases: oracle 1001, candidate 999"},
		"a candidate that shows no id": {bases(1001, 1001, true, true, false), bases(0, 0, false, false, false), ""},
		"an oracle that shows no id":   {bases(0, 1001, false, true, false), bases(1001, 1001, true, true, false), "oracle: its unique ids count from 0 (seen: false)"},
		"an oracle before the chart":   {bases(999, 999, true, true, false), bases(999, 999, true, true, false), "oracle: its unique ids count from 999"},
		"an oracle long after it":      {bases(1011, 1011, true, true, false), bases(1011, 1011, true, true, false), "oracle: its unique ids count from 1011"},
		// the fallback's ground, whatever the candidate
		"the oracle's bases are two": {bases(1001, 1002, true, true, false), bases(1001, 1002, true, true, false), "harness: the oracle's unique ids count from 1001, its alarm ids from 1002"},
		// a candidate without a log has one base, the alarm ids': bound as any
		"no log, the same second":   {bases(1001, 1001, true, true, false), bases(0, 1001, false, true, true), ""},
		"no log, two seconds later": {bases(1001, 1001, true, true, false), bases(0, 1003, false, true, true), ""},
		"no log, seven ids later":   {bases(1001, 1001, true, true, false), bases(0, 1008, false, true, true), "the alarm ids' bases: oracle 1001, candidate 1008"},
		"no log, no alarm id":       {bases(1001, 1001, true, true, false), bases(0, 0, false, false, true), ""},
		"no log, the oracle's two":  {bases(1002, 1001, true, true, false), bases(0, 1001, false, true, true), "harness: the oracle's unique ids count from 1002, its alarm ids from 1001"},
	} {
		err := healthBases([2]*healthNorm{c.oracle, c.candidate}, 1000)
		if (err == nil) != (c.want == "") || (err != nil && !strings.Contains(err.Error(), c.want)) {
			t.Errorf("the bases, %s: %v, want %q", name, err, c.want)
		}
	}

	// health.log without an alert log: a transition id prints as the rank of its first appearance in the file, the
	// nil id as it is; the unique id by the alarm ids' base
	ranked := blank()
	ranked.noLog = true
	ranked.anchor(501, 1)
	lines = fmt.Sprintf(record, strings.Repeat("a", 32), "0") + fmt.Sprintf(record, strings.Repeat("b", 32), "12") +
		fmt.Sprintf(record, strings.Repeat("a", 32), "3") + fmt.Sprintf(record, strings.Repeat("0", 32), "0")
	if err := os.WriteFile(filepath.Join(run, "log", "health.log"), []byte(lines), 0o644); err != nil {
		t.Fatal(err)
	}
	rankedLine := "time=T comm=netdata source=health level=warning tid=N thread=HEALTH alert_id=a+1 alert_unique_id=u+501 alert_transition_id=%s alert_duration=%s alert=a msg=x"
	wantLog = []string{fmt.Sprintf(rankedLine, "t#1", "0"), fmt.Sprintf(rankedLine, "t#2", "T"), fmt.Sprintf(rankedLine, "t#1", "T"),
		fmt.Sprintf(rankedLine, "00000000-0000-0000-0000-000000000000", "0")}
	if got := ranked.healthLogRanked(t, d); !slices.Equal(got, wantLog) {
		t.Errorf("health.log, ranked:\n got %q\nwant %q", got, wantLog)
	}
	// the same file through the alert log's names: no entry showed these ids
	if got := ranked.healthLog(t, d); !strings.Contains(got[0], " alert_transition_id=t? ") || !strings.Contains(got[2], " alert_transition_id=t? ") {
		t.Errorf("health.log, looked up: %q", got)
	}

	// the alert members of a v2 data answer: the two versions, the summary's alerts, each node's, context's and
	// instance's counts under their short or long key; nothing else of the answer
	data := `{"api":2,"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":5,"contexts_soft_hash":9,` +
		`"alerts_hard_hash":4,"alerts_soft_hash":12},"summary":{"nodes":[{"mg":"g1","nd":"n1","nm":"h","ni":0,"al":{"cl":2,"wr":1},"sts":{"min":1}}],` +
		`"contexts":[{"id":"c","is":{"sl":2},"al":{"cl":2,"wr":1}}],"instances":[{"id":"c.a","ni":0,"al":{"cl":2,"wr":1}},{"id":"c.b","ni":0},` +
		`{"id":"c.c","alerts":{"other":1}}],"alerts":[{"nm":"x","cl":2},{"nm":"y","wr":1}]},"result":{"data":[[1790000000,1]]}}`
	wantData := "versions.alerts_hard_hash: 4\nversions.alerts_soft_hash: 12\n" +
		`summary.alerts: [{"nm":"x","cl":2},{"nm":"y","wr":1}]` + "\n" +
		`summary.nodes[0] mg="g1": {"cl":2,"wr":1}` + "\n" + `summary.contexts[0] id="c": {"cl":2,"wr":1}` + "\n" +
		`summary.instances[0] id="c.a": {"cl":2,"wr":1}` + "\n" + `summary.instances[1] id="c.b": none` + "\n" +
		`summary.instances[2] id="c.c": {"other":1}`
	if got := healthDataAlerts([]byte(data)); got != wantData {
		t.Errorf("the alert members of a v2 data answer:\n got %s\nwant %s", got, wantData)
	}
	if got, want := healthDataAlerts([]byte(`{"versions":{},"summary":{}}`)), "versions.alerts_hard_hash: none\nversions.alerts_soft_hash: none\nsummary.alerts: none"; got != want {
		t.Errorf("a v2 data answer without alert members:\n got %s\nwant %s", got, want)
	}
	if got := healthDataAlerts([]byte("Unsupported API command")); !strings.HasPrefix(got, "not a JSON object") {
		t.Errorf("an answer that is no JSON: %s", got)
	}

	// the guard on /api/v1/alarms_values: each alert's status by its key, and no other alert
	values := "HTTP 200, application/json\n{\n\t\"hostname\": \"h\",\n\t\"alarms\": {\n\t\t\"c.a.x\": {\n\t\t\t\"id\": a+1,\n\t\t\t\"value\":70,\n" +
		"\t\t\t\"last_updated\":T,\n\t\t\t\"status\": \"WARNING\"\n\t\t},\n\t\t\"c.a.y\": {\n\t\t\t\"id\": a+2,\n\t\t\t\"value\":null,\n" +
		"\t\t\t\"last_updated\":0,\n\t\t\t\"status\": \"UNDEFINED\"\n\t\t}\n\t}\n}\n"
	for name, c := range map[string]struct {
		want map[string]string
		ok   bool
	}{
		"both":        {map[string]string{"c.a.x": "WARNING", "c.a.y": "UNDEFINED"}, true},
		"one missing": {map[string]string{"c.a.x": "WARNING"}, false},
		"one other":   {map[string]string{"c.a.x": "WARNING", "c.a.y": "CLEAR"}, false},
		"none wanted": {map[string]string{}, false},
	} {
		if err := healthValuesWant(c.want)(values); (err == nil) != c.ok {
			t.Errorf("the guard on alarms_values, %s: %v", name, err)
		}
	}
	if err := healthValuesWant(map[string]string{})("HTTP 404, text/plain\nUnsupported API command: alarms_values"); err == nil {
		t.Errorf("the guard on alarms_values passes a 404")
	}

	// a passing comparison logs a transcript without its environment; a failing one keeps the lines that differ
	o, c := "call 1: argv[0]=x\ncall 1: env A=1\ncall 1: env B=2", "call 1: argv[0]=x\ncall 1: env A=1\ncall 1: env B=3"
	if got := healthBrief(o); got != "call 1: argv[0]=x\n(and 2 environment lines)" {
		t.Errorf("brief %q", got)
	}
	if po, pc := healthUnlike(o, c); po != "call 1: argv[0]=x\ncall 1: env B=2" || pc != "call 1: argv[0]=x\ncall 1: env B=3" {
		t.Errorf("unlike %q and %q", po, pc)
	}
}
