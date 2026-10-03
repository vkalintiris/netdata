// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"path/filepath"
	"regexp"
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
//   - a transition id is a random UUID (health/health_log.c:225): named after its entry's rebased unique id (`t+k`), so
//     the same transition is the same name in the alert log, the notifier's arguments and health.log;
//   - every time field is a wall-clock second: `T` when set, 0 kept (set or not is compared), a lookup's window as its
//     length (`db_before` as `T+<n>`);
//   - each side's run directory and runtime directory: `{run}`, `{rt}`.

// healthNorm is one side's normalizer state.
type healthNorm struct {
	run string
	// uBase and aBase are the side's id bases: an id prints as its distance from its base; uSet and aSet once known
	uBase, aBase int64
	uSet, aSet   bool
	// tids are the side's transition ids by the unique id of their entries
	tids map[string]int64
}

func newHealthNorm(d *daemon.Daemon) *healthNorm {
	return &healthNorm{run: d.Opts.RunDir, tids: map[string]int64{}}
}

var (
	healthUniqueRe = regexp.MustCompile(`"(unique_id|latest_alarm_log_unique_id|updated_by_id|updates_id)":\s*(\d+)`)
	healthAlarmRe  = regexp.MustCompile(`"(alarm_id|id)":\s*(\d+)`)
	// an alert log entry's unique id, then its transition id (sqlite_health.c:1160-1164: the members between are the
	// alarm id, the event id and the config hash)
	healthEntryRe = regexp.MustCompile(`"unique_id":\s*(\d+),\s*"alarm_id":\s*\d+,\s*"alarm_event_id":\s*\d+,\s*"config_hash_id":\s*"[^"]*",\s*"transition_id":\s*"([0-9a-f-]{36})"`)
	healthTidRe   = regexp.MustCompile(`"transition_id":\s*"([0-9a-f-]{36})"`)
	// the time members of /api/v1/alarms and /api/v1/alarm_log (health/health_json.c:53-103, sqlite_health.c:1160-1200);
	// last_repeat is a quoted number in the first and a number in the second
	healthTimeRe = regexp.MustCompile(`"(now|when|exec_run|delay_up_to_timestamp|last_repeat|last_status_change|last_updated|next_update|duration|non_clear_duration)":(\s*)("?)(\d+)("?)`)
	healthDBRe   = regexp.MustCompile(`"db_after":(\s*)(\d+),(\s*)"db_before":(\s*)(\d+)`)
)

// observe takes what a body shows of the side's ids: a base is the smallest id seen, less one (the first id is the
// seed plus one; healthPair.settle waits for the alert log's first entries, so the bases are the seeds before
// anything is compared), and each entry's transition id is tied to its unique id.
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
	for _, m := range healthEntryRe.FindAllStringSubmatch(body, -1) {
		v, _ := strconv.ParseInt(m[1], 10, 64)
		n.tids[m[2]] = v
	}
}

// unique and alarm print an id as its distance from the side's base.
func (n *healthNorm) unique(v int64) string { return healthRebase("u", v, n.uBase, n.uSet) }
func (n *healthNorm) alarm(v int64) string  { return healthRebase("a", v, n.aBase, n.aSet) }

func healthRebase(prefix string, v, base int64, set bool) string {
	switch {
	case v == 0:
		return "0"
	case !set:
		return prefix + "?"
	}
	return fmt.Sprintf("%s%+d", prefix, v-base)
}

// tid names a transition id after its entry's unique id; one no entry showed prints `t?`.
func (n *healthNorm) tid(uuid string) string {
	if v, ok := n.tids[uuid]; ok {
		return "t" + strings.TrimPrefix(n.unique(v), "u")
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
	healthArgWarnList = 24
	healthArgCritList = 25
	healthArgGUID     = 28
	healthArgTid      = 29
)

var healthListEpochRe = regexp.MustCompile(`=\d+`)

// call renders one notifier call as two agents must agree on it: its arguments (arg), the environment (the side's
// directories replaced, then maskEnv: each agent's invocation id), the directory, the parent's name, the rule it
// matched and how it ended. The call's number is its position in the transcript.
func (n *healthNorm) call(c notify.Call) []string {
	if len(c.Argv) > healthArgTid {
		if v, err := strconv.ParseInt(c.Argv[healthArgUnique], 10, 64); err == nil {
			n.tids[c.Argv[healthArgTid]] = v
		}
	}
	out := []string{}
	for i, a := range c.Argv {
		out = append(out, fmt.Sprintf("argv[%d]=%s", i, n.arg(i, a)))
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

// arg renders a notification's argument i (argv[0] is the script): the ids rebased, the transition id named, the
// clock masked (the transition's time, the two durations, and each `name=<last status change>` item of the
// raised-alert lists, health_notifications.c:348), the side's directories replaced.
func (n *healthNorm) arg(i int, a string) string {
	switch i {
	case healthArgUnique:
		if v, err := strconv.ParseInt(a, 10, 64); err == nil {
			a = n.unique(v)
		}
	case healthArgAlarm:
		if v, err := strconv.ParseInt(a, 10, 64); err == nil {
			a = n.alarm(v)
		}
	case healthArgWhen, healthArgDuration, healthArgNonClear:
		if a != "0" {
			a = "T"
		}
	case healthArgWarnList, healthArgCritList:
		a = healthListEpochRe.ReplaceAllString(a, "=T")
	case healthArgTid:
		a = n.tid(a)
	}
	return n.paths(a)
}

// healthCommandRe is a notification's command in a record (the spawn server's, about a call that failed or was
// killed): `exec '<script>' '<a1>' … '<a33>'`, up to the quote that closes the logged `/bin/sh -c "…"`.
var healthCommandRe = regexp.MustCompile(`exec '(.*)'\\"`)

// command renders the notification's command inside a record with each argument as arg does.
func (n *healthNorm) command(record string) string {
	return healthCommandRe.ReplaceAllStringFunc(record, func(m string) string {
		words := strings.Split(healthCommandRe.FindStringSubmatch(m)[1], "' '")
		for i, w := range words {
			words[i] = n.arg(i, w)
		}
		return "exec '" + strings.Join(words, "' '") + `'\"`
	})
}

// calls renders a side's notifier transcript in call order.
func (n *healthNorm) calls(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	calls, err := notify.Calls(d.Opts.RunDir)
	if err != nil {
		t.Fatal(err)
	}
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
// transition id, duration and notification time normalized and the side's directories replaced.
func (n *healthNorm) healthLog(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "health.log") {
		if m := healthLogUniqueRe.FindStringSubmatch(l); m != nil {
			if tid := healthLogTidRe.FindStringSubmatch(l); tid != nil {
				v, _ := strconv.ParseInt(m[1], 10, 64)
				n.tids[healthDashed(tid[1])] = v
			}
		}
		l = healthLogUniqueRe.ReplaceAllStringFunc(l, func(m string) string {
			v, _ := strconv.ParseInt(strings.TrimPrefix(m, " alert_unique_id="), 10, 64)
			return " alert_unique_id=" + n.unique(v)
		})
		l = healthLogAlarmRe.ReplaceAllStringFunc(l, func(m string) string {
			v, _ := strconv.ParseInt(strings.TrimPrefix(m, " alert_id="), 10, 64)
			return " alert_id=" + n.alarm(v)
		})
		l = healthLogTidRe.ReplaceAllStringFunc(l, func(m string) string {
			return " alert_transition_id=" + n.tid(healthDashed(strings.TrimPrefix(m, " alert_transition_id=")))
		})
		l = healthLogTimeRe.ReplaceAllString(l, " ${1}=T")
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
	healthSQLTimeRe   = regexp.MustCompile(` (when_key|duration|non_clear_duration|exec_run_timestamp|delay_up_to_timestamp|last_repeat|date_updated|date_scheduled|date_submitted|date_cloud_ack)=(\d+)`)
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
// their distance from the side's base (0 kept), a transition id by its entry's unique id, a set time as T (0 kept), a
// lookup's window by its length.
func TestHealthNorm(t *testing.T) {
	n := &healthNorm{run: "/ndt/x/oracle", tids: map[string]int64{}}
	tid := "11111111-2222-4333-8444-555555555555"
	log := `[{"unique_id":1001,"alarm_id":501,"alarm_event_id":2,"config_hash_id":"aaaaaaaa-0000-4000-8000-000000000001",` +
		`"transition_id":"` + tid + `","name":"a","exec":"/ndt/x/oracle/notify/stub","when":1790000000,"duration":0,` +
		`"non_clear_duration":7,"delay":0,"delay_up_to_timestamp":1790000000,"updated_by_id":0,"updates_id":1000,` +
		`"last_repeat":0,"value":70}]`
	want := `[{"unique_id":u+2,"alarm_id":a+1,"alarm_event_id":2,"config_hash_id":"aaaaaaaa-0000-4000-8000-000000000001",` +
		`"transition_id":"t+2","name":"a","exec":"{run}/notify/stub","when":T,"duration":0,` +
		`"non_clear_duration":T,"delay":0,"delay_up_to_timestamp":T,"updated_by_id":0,"updates_id":u+1,` +
		`"last_repeat":0,"value":70}]`
	// the base is the lowest unique id seen, less one: an older entry (the link's, stored later) moves it
	n.observe(`{"unique_id":1000,"alarm_id":501}`)
	if got := n.json(log); got != want {
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
	args := map[int][2]string{
		healthArgUnique:   {"1001", "u+2"},
		healthArgAlarm:    {"501", "a+1"},
		5:                 {"2", "2"},
		healthArgWhen:     {"1790000000", "T"},
		healthArgDuration: {"0", "0"},
		healthArgNonClear: {"7", "T"},
		13:                {"line=2,file=/ndt/x/oracle/etc/health.d/parity.conf", "line=2,file={run}/etc/health.d/parity.conf"},
		healthArgWarnList: {"b=1790000003,a=1790000002", "b=T,a=T"},
		healthArgTid:      {tid, "t+2"},
		30:                {"1790000000", "1790000000"},
	}
	for i, c := range args {
		if got := n.arg(i, c[0]); got != c[1] {
			t.Errorf("arg %d %q: got %q, want %q", i, c[0], got, c[1])
		}
	}
	if got := n.tid("99999999-2222-4333-8444-555555555555"); got != "t?" {
		t.Errorf("an unknown transition id prints %q", got)
	}
	record := `msg="SPAWN SERVER: child with pid P exited with exit code 3: /bin/sh -c \"exec '<RUN>/notify/stub' 'root' 'h' '1001' '501' '2' '1790000000' 'a'\""`
	wantRecord := `msg="SPAWN SERVER: child with pid P exited with exit code 3: /bin/sh -c \"exec '<RUN>/notify/stub' 'root' 'h' 'u+2' 'a+1' '2' 'T' 'a'\""`
	if got := n.command(record); got != wantRecord {
		t.Errorf("command:\n got %s\nwant %s", got, wantRecord)
	}
	// a side that showed no id yet prints `?`, never a number another side could match by chance
	if got := (&healthNorm{tids: map[string]int64{}}).unique(1001); got != "u?" {
		t.Errorf("an id without a base prints %q", got)
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
