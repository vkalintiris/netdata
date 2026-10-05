// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/notify"
)

// healthCalcConf is one alert on the fake plugin's chart: the last value of `a`, WARNING above 50, CRITICAL above 90.
const healthCalcConf = `# the health checks' one alert on hsig.values
 alarm: hs_calc
    on: hsig.values
  calc: $a
 every: 1s
  warn: $this > 50
  crit: $this > 90
 units: things
  info: the last value of a
`

// healthCalcHold is how long each value is held for a `calc` alert: both agents' passes read it within 3 s (plan §5.3).
const healthCalcHold = 4 * time.Second

// healthPairConf are the `pair` case's alerts (D193 point 1: more than one alert notified in a pass). The four on the
// collected chart read the same value in the same pass, so at 70 three go WARNING or CRITICAL together and each call
// names the others in its lists of raised alerts (arguments 22 to 25), in the order the agent's sort leaves alerts
// that changed in one second; the file's order is not the names' order, so a list sorted by name shows.
//   - hp_a: WARNING above 50, CRITICAL above 90: the one alert that changes while the others stay raised;
//   - hp_d, hp_b: WARNING above 60; hp_b's recipient is `silent`, which is the stock script's word and nothing to the
//     agent: the call is made, with the word as its first argument;
//   - hp_c: CRITICAL above 60 and no warning: its call names its critical expression, and nothing once it is CLEAR;
//   - hp_idle: on a chart that exists and is never collected: linked, never evaluated, in no list.
const (
	healthPairEmit = `CHART hp.idle '' 'title' 'units' 'family' 'hp.ictx' line 1000 1 '' '' ''
DIMENSION a '' absolute 1 1
`
	healthPairConf = `# the pair case's alerts
 alarm: hp_a
    on: hp.values
  calc: $a
 every: 1s
  warn: $this > 50
  crit: $this > 90
 units: things
  info: warning above 50, critical above 90

 alarm: hp_d
    on: hp.values
  calc: $a
 every: 1s
  warn: $this > 60
 units: things
  info: warning above 60

 alarm: hp_c
    on: hp.values
  calc: $a
 every: 1s
  crit: $this > 60
 units: things
  info: critical above 60, no warning

 alarm: hp_b
    on: hp.values
  calc: $a
 every: 1s
  warn: $this > 60
    to: silent
 units: things
  info: warning above 60, to nobody

 alarm: hp_idle
    on: hp.idle
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: on a chart that is never collected
`
)

// healthNoClearConf is the `no-clear` case's alert: its return to CLEAR is never notified, and its raised statuses
// are notified without a look at what was notified before (health_notifications.c:398-437).
const healthNoClearConf = `# the no-clear case's alert on hsig.values
 alarm: hn_calc
    on: hsig.values
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: the last value of a, never notified when it clears
options: no-clear-notification
`

// healthDedupConf is the `dedup` case's alert: WARNING above 50, and at 80 its warning divides by zero, which C's
// evaluator takes for an error (libnetdata/eval/eval-evaluate.c:152-177, :301-332): with no other threshold the alert
// is UNDEFINED there. So it goes WARNING, UNDEFINED, WARNING: the second WARNING is the status that was last
// notified, and is not notified again (health_notifications.c:414-425).
const healthDedupConf = `# the dedup case's alert on hsig.values
 alarm: hdup_calc
    on: hsig.values
  calc: $a
 every: 1s
  warn: ($this > 50) / ($this != 80)
 units: things
  info: the last value of a, undefined at 80
`

// healthMissingConf are the `missing` case's alerts: one notified by the stub, one whose rule names a notifier of its
// own that does not exist (in the side's notifier directory: healthPrepare). The agent hands the name to the shell as
// any other, and takes the shell's failure for the notification's.
const healthMissingConf = `# the missing case's alerts on hsig.values
 alarm: hm_ok
    on: hsig.values
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: notified by the host's notifier

 alarm: hm_absent
    on: hsig.values
  calc: $a
 every: 1s
  warn: $this > 50
  exec: {run}/notify/absent
 units: things
  info: notified by a program that does not exist
`

// execView is side i's alert log as each entry's notification state: `name: OLD->NEW exec_run=T|0 exec_code=N
// exec_failed=… processed=… updated=… silenced=… last_repeat=T|0`, oldest first. An entry with `last_repeat` set is a
// repeat's, which no other entry replaces.
func (h *healthPair) execView(i int) string {
	entries, raw, err := h.logAs(h.n[i], i, "/api/v1/alarm_log")
	if err != nil {
		return err.Error()
	}
	set := func(v int64) string {
		if v != 0 {
			return "T"
		}
		return "0"
	}
	var out []string
	for _, e := range entries {
		out = append(out, fmt.Sprintf("%s: %s->%s exec_run=%s exec_code=%d exec_failed=%t processed=%t updated=%t silenced=%t last_repeat=%s",
			e.Name, e.OldStatus, e.Status, set(e.ExecRun), e.ExecCode, e.ExecFailed, e.Processed, e.Updated, e.Silenced, set(e.LastRepeat)))
	}
	return h.keep(i, strings.Join(out, "\n"), raw)
}

// HEALTH's records about a notification (health_notifications.c): the wait's (:24-82: a notification killed, a wait
// for one that never started), the two of a command that was not run (:518-527) and, at debug level, what was decided
// for each due entry (:398-451: sent, or why not).
var healthNotifyRecordRe = regexp.MustCompile(`msg="(HEALTH: alert notification|attempted to wait for the execution|` +
	`Failed to execute alarm notification|Failed to format command arguments|` +
	`\[[^\]]*\]: (Sending notification for alarm|Health not sending (again )?notification for alarm))`)

// healthNotifyRecords are a side's records about its notifications: HEALTH's in daemon.log, then every collector.log
// line naming the side's notifier directory (the spawn server's and the spawn client's records, and what the shell
// said about a notifier it could not run), pids and request numbers masked, the command's arguments rendered as the
// transcripts' (healthNorm.command). `stopping` is for a case that stops the agents while a notifier runs: the
// `errno` field is left out (a record written then carries the errno of the wait the stop interrupted, or of the one
// before it), and of collector.log only the agent's own records are taken: what the spawn server writes about a
// process the agent gave up waiting for comes from another process, at a moment of its own.
func healthNotifyRecords(t *testing.T, n *healthNorm, d *daemon.Daemon, stopping bool) []string {
	t.Helper()
	var out []string
	add := func(file, l string) {
		l = n.command(normalizeLog(l, d.Opts.RunDir, ""))
		if stopping {
			l = errnoRe.ReplaceAllString(l, "")
		}
		l = pidRe.ReplaceAllString(l, "pid P")
		out = append(out, file+" "+requestRe.ReplaceAllString(l, "request R"))
	}
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if healthNotifyRecordRe.MatchString(l) {
			add("daemon.log", l)
		}
	}
	for _, l := range logLines(t, d.Opts.RunDir, "collector.log") {
		if strings.Contains(l, notify.Dir(d.Opts.RunDir)+"/") && (!stopping || strings.Contains(l, " comm=netdata ")) {
			add("collector.log", l)
		}
	}
	return out
}

// transcript is a view: side i's notifier transcript (healthNorm.calls).
func (h *healthPair) transcript(t *testing.T) func(i int) string {
	return func(i int) string { return strings.Join(h.n[i].calls(t, h.p.Each()[i].Daemon), "\n") }
}

// healthCall is one call of a transcript as a guard reads it: its arguments by position, and its other lines but the
// environment's (`cwd …`, `parent …`, `fd …`, `rule …`, `stdout …`, `end …`).
type healthCall struct {
	argv  map[int]string
	lines []string
}

var healthCallLineRe = regexp.MustCompile(`^call (\d+): (?:argv\[(\d+)\]=)?(.*)$`)

// healthCallsOf reads a transcript back into its calls, in its order.
func healthCallsOf(transcript string) []healthCall {
	var out []healthCall
	for _, l := range strings.Split(transcript, "\n") {
		m := healthCallLineRe.FindStringSubmatch(l)
		if m == nil {
			continue
		}
		k, _ := strconv.Atoi(m[1])
		for len(out) < k {
			out = append(out, healthCall{argv: map[int]string{}})
		}
		switch c := &out[k-1]; {
		case m[2] != "":
			i, _ := strconv.Atoi(m[2])
			c.argv[i] = m[3]
		case !strings.HasPrefix(m[3], "env "):
			c.lines = append(c.lines, m[3])
		}
	}
	return out
}

// is reports whether the call is for the alert's transition to the status.
func (c healthCall) is(name, status string) bool {
	return c.argv[notify.ArgName] == name && c.argv[notify.ArgStatus] == status
}

// want checks arguments of the call by position.
func (c healthCall) want(args map[int]string) error {
	for _, i := range slices.Sorted(maps.Keys(args)) {
		if c.argv[i] != args[i] {
			return fmt.Errorf("the call for %s %s has argv[%d]=%q, want %q", c.argv[notify.ArgName], c.argv[notify.ArgStatus], i, c.argv[i], args[i])
		}
	}
	return nil
}

// healthCallsWant is a guard on a transcript: it holds n calls, and each of `checks` passes on them.
func healthCallsWant(n int, checks ...func(calls []healthCall) error) func(string) error {
	return func(transcript string) error {
		calls := healthCallsOf(transcript)
		if len(calls) != n {
			return fmt.Errorf("%d calls, want %d", len(calls), n)
		}
		for _, check := range checks {
			if err := check(calls); err != nil {
				return err
			}
		}
		return nil
	}
}

// healthCallFor is a check for healthCallsWant: the k-th call (from 1) for the alert's transition to the status
// exists, has the arguments, and holds each of the lines.
func healthCallFor(name, status string, k int, args map[int]string, lines ...string) func([]healthCall) error {
	return func(calls []healthCall) error {
		seen := 0
		for _, c := range calls {
			if !c.is(name, status) {
				continue
			}
			if seen++; seen < k {
				continue
			}
			if err := c.want(args); err != nil {
				return err
			}
			for _, l := range lines {
				if !slices.Contains(c.lines, l) {
					return fmt.Errorf("the call for %s %s has no line %q: %q", name, status, l, c.lines)
				}
			}
			return nil
		}
		return fmt.Errorf("%d calls for %s %s, want %d or more", seen, name, status, k)
	}
}

// healthCallsEnded is a check for healthCallsWant: every call ended by itself with the exit code.
func healthCallsEnded(code int) func([]healthCall) error {
	return func(calls []healthCall) error {
		for k, c := range calls {
			if want := fmt.Sprintf("end exit %d", code); !slices.Contains(c.lines, want) {
				return fmt.Errorf("call %d has no %q: %q", k+1, want, c.lines)
			}
		}
		return nil
	}
}

// processed waits until the newest entry the oracle's alert log holds for an alert takes it to `status` and has been
// through the notification scan (`processed`): what the scan made of the entry, a call or none, is then on the
// record, so a transcript without a new call is a verdict and not an early read. The candidate gets the bounded wait
// to show the same, without a verdict (one that serves no alert log is not waited for).
func (h *healthPair) processed(t *testing.T, what, name, status string) {
	t.Helper()
	h.processedAs(t, h.n, "", what, name, status)
}

// processedAs is processed for the alert log of another host (`/host/<name>`), with that host's normalizers.
func (h *healthPair) processedAs(t *testing.T, n [2]*healthNorm, prefix, what, name, status string) {
	t.Helper()
	newest := func(i int) error {
		entries, err := h.entriesAs(n[i], i, prefix+"/api/v1/alarm_log")
		if err != nil {
			return err
		}
		for k := len(entries) - 1; k >= 0; k-- {
			if e := entries[k]; e.Name == name {
				if e.Status != status || !e.Processed {
					return fmt.Errorf("the newest entry of %s is %s->%s processed=%t, want %s, processed", name, e.OldStatus, e.Status, e.Processed, status)
				}
				return nil
			}
		}
		return fmt.Errorf("no entry of %s", name)
	}
	h.waitOracle(t, what+": the alert log", func() (string, error) { return "", newest(0) })
	for end := time.Now().Add(healthCandidateWait); time.Now().Before(end) && newest(1) != nil; time.Sleep(250 * time.Millisecond) {
		if r := healthGet(h.p.Candidate, prefix+"/api/v1/alarm_log"); r.Status != 200 {
			return
		}
	}
}

// healthRecordsWant is a guard on a side's records about its notifications: each part is held by `count` records
// (-1: by one or more).
func healthRecordsWant(want map[string]int) func([]string) error {
	return func(records []string) error {
		for _, part := range slices.Sorted(maps.Keys(want)) {
			n := 0
			for _, r := range records {
				if strings.Contains(r, part) {
					n++
				}
			}
			if count := want[part]; (count < 0 && n == 0) || (count >= 0 && n != count) {
				return fmt.Errorf("%d records hold %q, want %d (-1: one or more)", n, part, count)
			}
		}
		return nil
	}
}

// healthHas is a guard on a view: it holds each part.
func healthHas(parts ...string) func(string) error {
	return func(view string) error {
		for _, p := range parts {
			if !strings.Contains(view, p) {
				return fmt.Errorf("no entry holds %q", p)
			}
		}
		return nil
	}
}

// the texts HEALTH's decision records hold (health_notifications.c:398-451)
const (
	healthSent      = "]: Sending notification for alarm '"
	healthNotAgain  = "]: Health not sending again notification for alarm '"
	healthNoClear   = "(it has no-clear-notification enabled)"
	healthSilenced  = "(command API has disabled notifications)"
	healthPastLimit = "is still running past its execution timeout - killing it"
)

// TestHealthNotify (check `health.notify`, M9 commit 0, D183; the cases from `pair` on: M9 commit 6, D208): alerts
// through their statuses with the recording notifier. The notifier's transcripts are compared (every argument, the
// environment, the directory, the parent, the descriptors, how each call ended: healthNorm.call), then the alert log's
// notification state and, both stopped, the agents' records about the notifications. Cases:
//   - `basic`: one alert through CLEAR, WARNING, CRITICAL and CLEAR; `fail` has the CRITICAL call exit 3; `timeout`
//     has it sleep past a 2 s execution timeout ignoring SIGTERM, so the agent kills it;
//   - `pair`: four alerts that change in one pass (healthPairConf): the lists of the other raised alerts, each alert's
//     own expression, a recipient of its own, and the order C starts a pass's notifications in (its debug records);
//   - `repeat`: an alert that repeats while raised: the repeats' calls, which HEALTH waits for one by one inside its
//     pass, their entries in the alert log and, both stopped, the health tables and the unclaimed queue's records;
//   - `no-clear`: an alert with `no-clear-notification`: its CLEAR is never notified, its WARNING always;
//   - `dedup`: an alert that returns to the status it was last notified for: not notified again;
//   - `missing`: a rule whose own notifier does not exist: the shell's failure is the notification's;
//   - `silenced`: an alert silenced through the management API while it rises: no call for the entry made meanwhile,
//     then or after the reset;
//   - `stdout`: a notifier that writes to its stdout after HEALTH began to wait for it: the write ends it, and the
//     agent counts a success;
//   - `exit`: both agents stop while HEALTH waits for a notifier that takes 30 s: what HEALTH and the spawn client
//     record, and what the entry's row keeps.
func TestHealthNotify(t *testing.T) {
	// calls is a guard on a transcript: it holds n calls, the last one for the status
	calls := func(n int, status string) func(string) error {
		return func(oracle string) error {
			if n == 0 {
				if oracle != "" {
					return fmt.Errorf("the notifier was called")
				}
				return nil
			}
			if !strings.Contains(oracle, fmt.Sprintf("call %d: argv[%d]=%s\n", n, notify.ArgStatus, status)) ||
				strings.Contains(oracle, fmt.Sprintf("call %d: ", n+1)) {
				return fmt.Errorf("want %d calls, the last for %s", n, status)
			}
			return nil
		}
	}
	// the first CLEAR (from UNINITIALIZED) is not notified (health_notifications.c:427-436): a call for each later status
	wantCalls := []int{0, 1, 2, 3}
	play := func(lastEnds string, execGuard func(view string) error) func(t *testing.T, h *healthPair) {
		return func(t *testing.T, h *healthPair) {
			transcript := h.transcript(t)
			healthPlaySig(t, h, healthCalcHold, func(k int) {
				if k == 0 {
					// no call yet on either side: only the oracle's state is waited for
					h.waitOracle(t, "phase 0: /api/v1/alarms?all", func() (string, error) {
						v := h.get(0, "/api/v1/alarms?all")
						return v, healthAll("CLEAR", "hs_calc")(v)
					})
					return
				}
				h.compareNow(t, fmt.Sprintf("phase %d: the notifier's calls", k), transcript, calls(wantCalls[k], healthSigStatus[k]))
			})
			h.compareNow(t, "the notifier's calls at the end", transcript, func(oracle string) error {
				if !strings.Contains(oracle, lastEnds) {
					return fmt.Errorf("no %q", lastEnds)
				}
				return nil
			})
			h.compareNow(t, "the alert log's notification state", h.execView, execGuard)
		}
	}
	// records compares the agents' records about the notifications, both stopped
	records := func(stopping bool, guard func([]string) error) func(t *testing.T, h *healthPair) {
		return func(t *testing.T, h *healthPair) {
			h.compareLines(t, "the records about the notifications",
				func(i int) []string { return healthNotifyRecords(t, h.n[i], h.p.Each()[i].Daemon, stopping) }, guard)
		}
	}
	after := func(want ...string) func(t *testing.T, h *healthPair) {
		parts := map[string]int{}
		for _, w := range want {
			parts[w] = -1
		}
		return records(false, healthRecordsWant(parts))
	}
	// statuses waits until the oracle's alerts have the statuses, then gives the candidate the bounded wait
	statuses := func(t *testing.T, h *healthPair, what string, want map[string]string) {
		t.Helper()
		all := func(i int) string { return h.get(i, "/api/v1/alarms?all") }
		h.waitOracle(t, what+": /api/v1/alarms?all", func() (string, error) {
			v := all(0)
			return v, healthWant(want)(v)
		})
		h.waitCandidate("/api/v1/alarms?all", all)
	}
	sc := healthValues("hsig.values", "hsig.ctx", []string{"a"}, healthSigPhases...)
	values := func(vs ...int64) []map[string]int64 {
		var out []map[string]int64
		for _, v := range vs {
			out = append(out, map[string]int64{"a": v})
		}
		return out
	}
	runHealthCases(t, map[string]healthCase{
		"basic": {
			conf: healthCalcConf, sc: sc,
			play: play("call 3: end exit 0", healthHas("hs_calc: CLEAR->WARNING exec_run=T exec_code=0 exec_failed=false",
				"hs_calc: WARNING->CRITICAL exec_run=T exec_code=0 exec_failed=false")),
			after: after(),
		},
		"fail": {
			conf: healthCalcConf, sc: sc,
			ctl:   notify.Control{Rules: []notify.Rule{{Alert: "hs_calc", Status: "CRITICAL", Exit: 3}}},
			play:  play("call 2: end exit 3", healthHas("hs_calc: WARNING->CRITICAL exec_run=T exec_code=3 exec_failed=true")),
			after: after("exited with exit code 3"),
		},
		"timeout": {
			conf: healthCalcConf, sc: sc,
			extra: "    notification execution timeout = 2s\n",
			ctl:   notify.Control{Rules: []notify.Rule{{Alert: "hs_calc", Status: "CRITICAL", SleepMs: 30000, IgnoreTerm: true}}},
			play:  play("call 2: end none", healthHas("hs_calc: WARNING->CRITICAL exec_run=T exec_code=128 exec_failed=true")),
			// a call without an end was killed or still runs: the spawn server's record says which
			after: after(healthPastLimit, "killed by signal 9"),
		},
		"pair": {
			conf: healthPairConf, logs: healthLogsDebug,
			sc: healthScenario(healthPairEmit, "hp.values", "hp.ctx", []string{"a"}, healthSigPhases...),
			play: func(t *testing.T, h *healthPair) {
				transcript := h.transcript(t)
				// the two lists of a call: nothing of the chart that is never collected
				lists := func(calls []healthCall) error {
					for _, c := range calls {
						if l := c.argv[healthArgWarnList] + "," + c.argv[healthArgCritList]; strings.Contains(l, "hp_idle") {
							return fmt.Errorf("the call for %s %s lists hp_idle: %q", c.argv[notify.ArgName], c.argv[notify.ArgStatus], l)
						}
					}
					return nil
				}
				// hp_a's call while the three others are raised: two WARNING (in the alerts' order: hp_d was defined
				// before hp_b, and both changed in one second) and one CRITICAL, each by the entry that raised it
				others := func(status string, k int, more map[int]string) func([]healthCall) error {
					return func(calls []healthCall) error {
						if err := healthCallFor("hp_a", status, k, more)(calls); err != nil {
							return err
						}
						for _, c := range calls {
							if !c.is("hp_a", status) {
								continue
							}
							warn, crit := strings.Split(c.argv[healthArgWarnList], ","), c.argv[healthArgCritList]
							if c.argv[healthArgWarnCount] != "2" || c.argv[healthArgCritCount] != "1" || len(warn) != 2 ||
								!strings.HasPrefix(warn[0], "hp_d=when(u+") || !strings.HasPrefix(warn[1], "hp_b=when(u+") ||
								!strings.HasPrefix(crit, "hp_c=when(u+") || strings.Contains(crit, ",") {
								return fmt.Errorf("the call for hp_a %s counts %s WARNING %q and %s CRITICAL %q, want hp_d then hp_b, and hp_c, each by its entry",
									status, c.argv[healthArgWarnCount], c.argv[healthArgWarnList], c.argv[healthArgCritCount], crit)
							}
						}
						return nil
					}
				}
				raised := map[string]string{"hp_a": "WARNING", "hp_d": "WARNING", "hp_c": "CRITICAL", "hp_b": "WARNING"}
				low := map[string]string{"hp_a": "CLEAR", "hp_d": "CLEAR", "hp_c": "CLEAR", "hp_b": "CLEAR"}
				healthPlaySig(t, h, healthCalcHold, func(k int) {
					at := fmt.Sprintf("phase %d", k)
					switch k {
					case 0:
						statuses(t, h, at, low)
					case 1:
						statuses(t, h, at, raised)
						h.compareNow(t, at+": the notifier's calls", transcript, healthCallsWant(4, lists,
							others("WARNING", 1, map[int]string{1: "root", 20: "_this > 50"}),
							healthCallFor("hp_d", "WARNING", 1, map[int]string{1: "root", 20: "_this > 60", healthArgWarnCount: "2", healthArgCritCount: "1"}),
							healthCallFor("hp_c", "CRITICAL", 1, map[int]string{1: "root", 20: "_this > 60", healthArgWarnCount: "3", healthArgCritCount: "0",
								healthArgCritList: ""}),
							healthCallFor("hp_b", "WARNING", 1, map[int]string{1: "silent", 20: "_this > 60", healthArgWarnCount: "2", healthArgCritCount: "1"})))
						// the four changes are one pass's on the oracle: one second, four ids in a row
						entries, err := h.entriesAs(h.n[0], 0, "/api/v1/alarm_log")
						if err != nil {
							t.Fatalf("oracle: %v", err)
						}
						var changes []healthEntry
						for _, e := range entries {
							if e.OldStatus == "CLEAR" && e.Status != "CLEAR" {
								changes = append(changes, e)
							}
						}
						if len(changes) != 4 || changes[3].When != changes[0].When || changes[3].UniqueID != changes[0].UniqueID+3 {
							t.Fatalf("oracle: the four alerts did not rise in one pass: %+v", changes)
						}
					case 2:
						raised["hp_a"] = "CRITICAL"
						statuses(t, h, at, raised)
						h.compareNow(t, at+": the notifier's calls", transcript, healthCallsWant(5, lists,
							others("CRITICAL", 1, map[int]string{20: "_this > 90"})))
					case 3:
						statuses(t, h, at, low)
						none := map[int]string{healthArgWarnCount: "0", healthArgCritCount: "0", healthArgWarnList: "", healthArgCritList: ""}
						h.compareNow(t, at+": the notifier's calls", transcript, healthCallsWant(9, lists,
							healthCallFor("hp_a", "CLEAR", 1, none), healthCallFor("hp_d", "CLEAR", 1, none),
							// an alert without a warning expression has none to name once it is no longer CRITICAL
							healthCallFor("hp_c", "CLEAR", 1, map[int]string{20: "", 21: "", healthArgWarnCount: "0", healthArgCritCount: "0"}),
							healthCallFor("hp_b", "CLEAR", 1, map[int]string{1: "silent"})))
					}
				})
				h.compareNow(t, "the notifier's calls at the end", transcript, healthCallsWant(9, healthCallsEnded(0)))
				h.compareNow(t, "the alert log's notification state", h.execView, healthHas(
					"hp_a: CLEAR->WARNING exec_run=T exec_code=0 exec_failed=false processed=true updated=true",
					"hp_a: WARNING->CRITICAL exec_run=T exec_code=0 exec_failed=false processed=true updated=true",
					"hp_c: CLEAR->CRITICAL exec_run=T exec_code=0 exec_failed=false processed=true updated=true",
					"hp_b: WARNING->CLEAR exec_run=T exec_code=0 exec_failed=false processed=true updated=false",
					"hp_idle: REMOVED->UNINITIALIZED exec_run=0"))
			},
			// at debug level HEALTH says what it decided for each due entry, in the order it took them (a pass's
			// newest entry first), and the spawn server each process it made
			after: records(false, healthRecordsWant(map[string]int{healthSent: 9, "]: Health not sending": 0,
				"SPAWN SERVER: process created with pid P: ": 9})),
		},
		"repeat": {
			conf: healthRepeatConf,
			// every switch at the same second of a 5 s window: a raised phase lasts 10 s on both sides, which a repeat
			// every 4 s divides into the same count whichever second each side's pass first saw it
			grid: healthSigGrid,
			sc:   healthValues("hr.values", "hr.ctx", []string{"a"}, healthSigPhases...),
			play: func(t *testing.T, h *healthPair) {
				transcript := h.transcript(t)
				// repeats is a check: the calls for the status are its transition's and `least` repeats or more; a
				// repeat's argument 15 is `arg15`
				repeats := func(status string, least int, arg15 string) func([]healthCall) error {
					return func(calls []healthCall) error {
						n := 0
						for _, c := range calls {
							if !c.is("hr_calc", status) {
								continue
							}
							if n++; n > 1 && c.argv[healthArgNonClear] != arg15 {
								return fmt.Errorf("repeat %d of %s has argv[%d]=%q, want %q", n-1, status, healthArgNonClear, c.argv[healthArgNonClear], arg15)
							}
						}
						if n < 1+least {
							return fmt.Errorf("%d calls for %s, want its transition's and %d repeats or more", n, status, least)
						}
						return nil
					}
				}
				last := func(status string) func([]healthCall) error {
					return func(calls []healthCall) error {
						if len(calls) == 0 || !calls[len(calls)-1].is("hr_calc", status) {
							return fmt.Errorf("the last call is not for %s", status)
						}
						return nil
					}
				}
				guard := func(checks ...func([]healthCall) error) func(string) error {
					return func(transcript string) error {
						calls := healthCallsOf(transcript)
						for _, check := range checks {
							if err := check(calls); err != nil {
								return err
							}
						}
						return nil
					}
				}
				h.create(t)
				statuses(t, h, "CLEAR", map[string]string{"hr_calc": "CLEAR"})
				// the first value until the first CLEAR's row of the unclaimed queue was moved on (healthQuietMoved)
				h.release(t, "p1", 1, healthQuietMoved)
				// the transition's call alone, before the first repeat: a raised entry of an alert that repeats gets its
				// duration as argument 15, where another alert's would get its non-clear duration, 0
				h.compareNow(t, "WARNING: the notifier's calls", transcript,
					healthCallsWant(1, healthCallFor("hr_calc", "WARNING", 1, map[int]string{healthArgNonClear: "duration"})))
				h.release(t, "p2", 2, 6*time.Second)
				// a repeat of WARNING: the entry's old status is CLEAR, so its non-clear duration is 0 and the argument is
				// the duration alone
				h.compareNow(t, "CRITICAL: the notifier's calls", transcript,
					guard(repeats("WARNING", 2, "duration"), healthCallFor("hr_calc", "CRITICAL", 1, nil), last("CRITICAL")))
				h.release(t, "p3", 3, 6*time.Second)
				// a repeat of CRITICAL: its old status is WARNING, so both durations are one number
				h.compareNow(t, "CLEAR again: the notifier's calls", transcript,
					guard(repeats("WARNING", 2, "duration"), repeats("CRITICAL", 2, "non_clear_duration"),
						healthCallFor("hr_calc", "CLEAR", 1, nil), last("CLEAR")))
				h.compareNow(t, "the notifier's calls at the end", transcript, guard(healthCallsEnded(0)))
				// a repeat's entry is saved once, when its notifier starts: it keeps `processed`, never gets a code and
				// is never replaced
				h.compareNow(t, "the alert log's notification state", h.execView, healthHas(
					"hr_calc: CLEAR->WARNING exec_run=T exec_code=0 exec_failed=false processed=true updated=true silenced=false last_repeat=0",
					"hr_calc: CLEAR->WARNING exec_run=T exec_code=0 exec_failed=false processed=true updated=false silenced=false last_repeat=T",
					"hr_calc: WARNING->CRITICAL exec_run=T exec_code=0 exec_failed=false processed=true updated=false silenced=false last_repeat=T",
					"hr_calc: CRITICAL->CLEAR exec_run=T exec_code=0 exec_failed=false processed=true updated=false silenced=false last_repeat=0"))
				time.Sleep(healthQueueHold)
			},
			after: func(t *testing.T, h *healthPair) {
				repeats := 0
				h.compareLines(t, "the health tables", func(i int) []string { return h.n[i].healthDump(t, h.p.Each()[i].Daemon) },
					func(oracle []string) error {
						// a repeat's row: never saved again (no SAVED), IS_REPEATING, EXEC_IN_PROGRESS, EXEC_RUN and PROCESSED
						// (health/health.h:10-20), replaced by no entry and replacing none
						repeats = 0
						for _, l := range oracle {
							if strings.HasPrefix(l, "row health_log_detail ") && strings.Contains(l, " last_repeat=T ") {
								if repeats++; !strings.Contains(l, " updated_by_id=0 updates_id=0 ") || !strings.Contains(l, " flags=197 ") ||
									!strings.Contains(l, " exec_code=0 ") {
									return fmt.Errorf("a repeat's row is not as it was inserted: %s", l)
								}
							}
						}
						// the three links, the first CLEAR, the three changes, and two repeats or more per raised phase; past
						// the hold every row of the queue was moved on
						if repeats < 4 {
							return fmt.Errorf("%d rows of repeats, want 4 or more", repeats)
						}
						return healthRowsWant(map[string]int{"alert_hash": 1, "health_log": 1, "health_log_detail": 7 + repeats,
							"alert_queue": 0, "aclk_queue": 1, "alert_version": 0, "alert_hash_cloud": 0})(oracle)
					})
				// the first CLEAR, the three changes, and each repeat: a repeat's insert makes a row of the queue as a
				// change's does, due at once
				h.compareLines(t, "the unclaimed queue's records, summed", func(i int) []string { return h.queueMoves(t, i) },
					func(oracle []string) error {
						if want := fmt.Sprintf("processed %d, queued %d", 4+repeats, 4+repeats); len(oracle) != 1 || oracle[0] != want {
							return fmt.Errorf("want %s", want)
						}
						return nil
					})
			},
		},
		"no-clear": {
			conf: healthNoClearConf, logs: healthLogsDebug,
			sc: healthValues("hsig.values", "hsig.ctx", []string{"a"}, values(10, 70, 10, 70)...),
			play: func(t *testing.T, h *healthPair) {
				transcript := h.transcript(t)
				want := []int{0, 1, 1, 2}
				status := []string{"CLEAR", "WARNING", "CLEAR", "WARNING"}
				healthPlayPhases(t, h, 4, healthCalcHold, func(k int) {
					at := fmt.Sprintf("phase %d", k)
					h.processed(t, at, "hn_calc", status[k])
					if k == 0 {
						return
					}
					h.compareNow(t, at+": the notifier's calls", transcript, healthCallsWant(want[k],
						healthCallFor("hn_calc", "WARNING", (k+1)/2, map[int]string{healthArgNonClear: "0"})))
				})
				h.compareNow(t, "the notifier's calls at the end", transcript, healthCallsWant(2, healthCallsEnded(0)))
				h.compareNow(t, "the alert log's notification state", h.execView, healthBoth(healthHas(
					"hn_calc: UNINITIALIZED->CLEAR exec_run=0 exec_code=0 exec_failed=false processed=true updated=true",
					"hn_calc: WARNING->CLEAR exec_run=0 exec_code=0 exec_failed=false processed=true updated=true"),
					healthTimes("hn_calc: CLEAR->WARNING exec_run=T exec_code=0 exec_failed=false processed=true", 2)))
			},
			// both CLEARs are refused by the option, the first one too; both WARNINGs are sent
			after: records(false, healthRecordsWant(map[string]int{healthNoClear: 2, healthSent: 2, healthNotAgain: 0})),
		},
		"dedup": {
			conf: healthDedupConf, logs: healthLogsDebug,
			sc: healthValues("hsig.values", "hsig.ctx", []string{"a"}, values(10, 60, 80, 90, 10)...),
			play: func(t *testing.T, h *healthPair) {
				transcript := h.transcript(t)
				want := []int{0, 1, 1, 1, 2}
				status := []string{"CLEAR", "WARNING", "UNDEFINED", "WARNING", "CLEAR"}
				healthPlayPhases(t, h, 5, healthCalcHold, func(k int) {
					at := fmt.Sprintf("phase %d", k)
					h.processed(t, at, "hdup_calc", status[k])
					if k == 0 {
						return
					}
					h.compareNow(t, at+": the notifier's calls", transcript, healthCallsWant(want[k],
						healthCallFor("hdup_calc", "WARNING", 1, map[int]string{10: "CLEAR"})))
				})
				h.compareNow(t, "the notifier's calls at the end", transcript, healthCallsWant(2,
					healthCallFor("hdup_calc", "CLEAR", 1, map[int]string{10: "WARNING"}), healthCallsEnded(0)))
				h.compareNow(t, "the alert log's notification state", h.execView, healthHas(
					"hdup_calc: CLEAR->WARNING exec_run=T exec_code=0 exec_failed=false processed=true updated=true",
					"hdup_calc: WARNING->UNDEFINED exec_run=0 exec_code=0 exec_failed=false processed=true updated=true",
					"hdup_calc: UNDEFINED->WARNING exec_run=0 exec_code=0 exec_failed=false processed=true updated=true",
					"hdup_calc: WARNING->CLEAR exec_run=T exec_code=0 exec_failed=false processed=true updated=false"))
			},
			after: records(false, healthRecordsWant(map[string]int{
				healthNotAgain + "hsig.values.hdup_calc' status WARNING": 1, healthSent: 2})),
		},
		"missing": {
			conf: healthMissingConf,
			sc:   healthValues("hsig.values", "hsig.ctx", []string{"a"}, values(10, 70, 10)...),
			play: func(t *testing.T, h *healthPair) {
				transcript := h.transcript(t)
				status := []string{"CLEAR", "WARNING", "CLEAR"}
				healthPlayPhases(t, h, 3, healthCalcHold, func(k int) {
					at := fmt.Sprintf("phase %d", k)
					h.processed(t, at, "hm_ok", status[k])
					h.processed(t, at, "hm_absent", status[k])
					if k == 0 {
						return
					}
					// the stub is called for hm_ok alone
					h.compareNow(t, at+": the notifier's calls", transcript, healthCallsWant(k,
						healthCallFor("hm_ok", status[k], 1, map[int]string{0: "{run}/notify/stub"})))
				})
				h.compareNow(t, "the notifier's calls at the end", transcript, healthCallsWant(2, healthCallsEnded(0)))
				// the shell's code is the notification's, in the row once the alert's next entry replaced this one
				h.compareNow(t, "the alert log's notification state", h.execView, healthHas(
					"hm_absent: CLEAR->WARNING exec_run=T exec_code=127 exec_failed=true processed=true updated=true",
					"hm_absent: WARNING->CLEAR exec_run=T exec_code=0 exec_failed=false processed=true updated=false",
					"hm_ok: CLEAR->WARNING exec_run=T exec_code=0 exec_failed=false processed=true updated=true"))
			},
			// the rule's name reached the shell as it is: the shell's own line (the script's stderr is the collectors'
			// log) and the spawn server's record of its exit code, once for WARNING and once for CLEAR
			after: records(false, healthRecordsWant(map[string]int{
				"exec: <RUN>/notify/absent: not found":                                       2,
				`exited with exit code 127: /bin/sh -c \"exec '<RUN>/notify/absent' 'root' `: 2})),
		},
		"silenced": {
			conf: healthCalcConf, logs: healthLogsDebug,
			sc: healthValues("hsig.values", "hsig.ctx", []string{"a"}, values(10, 70, 10, 70)...),
			play: func(t *testing.T, h *healthPair) {
				transcript := h.transcript(t)
				// manage sends a command of the management API to both sides. The oracle must take it; what the
				// candidate answers is `health.silencers`' to judge
				manage := func(cmd, want string) {
					t.Helper()
					for i := range 2 {
						got := h.plain(i, "/api/v1/manage/health?cmd="+cmd, "X-Auth-Token: "+healthKey)
						if i == 0 && got != "HTTP 200, text/plain; charset=utf-8\n"+want {
							t.Fatalf("oracle: cmd=%s answered\n%s", cmd, got)
						}
					}
				}
				flags := func(want string) {
					t.Helper()
					h.waitOracle(t, "the alert's flags", func() (string, error) {
						v := h.flags(0)
						if !strings.Contains(v, want) {
							return v, fmt.Errorf("want %q", want)
						}
						return v, nil
					})
				}
				h.create(t)
				h.processed(t, "phase 0", "hs_calc", "CLEAR")
				manage("SILENCE&alarm=hs_calc", "Alarm notifications silenced for alarms matching the selectors\nAlarm selector added\n")
				flags("hs_calc disabled=false silenced=true")
				// the alert rises while it is silenced: its entry is made silenced, and the scan runs nothing for it
				h.release(t, "p1", 1, healthCalcHold)
				h.processed(t, "silenced", "hs_calc", "WARNING")
				h.compareNow(t, "silenced: the notifier's calls", transcript, healthCallsWant(0))
				silenced := "hs_calc: CLEAR->WARNING exec_run=0 exec_code=0 exec_failed=false processed=true updated=false silenced=true"
				h.compareNow(t, "silenced: the alert log's notification state", h.execView, healthHas(silenced))
				// the reset does not bring the entry back: it was processed. Its CLEAR is then an alert's that was never
				// notified, and is not notified either
				manage("RESET", "All health checks and notifications are enabled\n")
				flags("hs_calc disabled=false silenced=false")
				h.release(t, "p2", 2, healthCalcHold)
				h.processed(t, "after the reset, CLEAR", "hs_calc", "CLEAR")
				h.compareNow(t, "after the reset, CLEAR: the notifier's calls", transcript, healthCallsWant(0))
				h.release(t, "p3", 3, healthCalcHold)
				h.compareNow(t, "after the reset, WARNING: the notifier's calls", transcript,
					healthCallsWant(1, healthCallFor("hs_calc", "WARNING", 1, nil), healthCallsEnded(0)))
				h.compareNow(t, "the alert log's notification state", h.execView, healthBoth(healthHas(
					strings.Replace(silenced, "updated=false", "updated=true", 1),
					"hs_calc: WARNING->CLEAR exec_run=0 exec_code=0 exec_failed=false processed=true updated=true silenced=false",
					"hs_calc: CLEAR->WARNING exec_run=T exec_code=0 exec_failed=false processed=true updated=false silenced=false"),
					healthTimes("silenced=true", 1)))
			},
			after: records(false, healthRecordsWant(map[string]int{healthSilenced: 1, healthSent: 1})),
		},
		"stdout": {
			conf: healthCalcConf, logs: healthLogsDebug, sc: sc,
			// HEALTH waits for the CRITICAL call from the end of the pass that started it, and closed its end of the
			// call's stdout to do so: the line written 3 s later has no reader
			ctl: notify.Control{Rules: []notify.Rule{{Alert: "hs_calc", Status: "CRITICAL", StdoutAfterMs: 3000}}},
			play: func(t *testing.T, h *healthPair) {
				transcript := h.transcript(t)
				written := healthCallFor("hs_calc", "CRITICAL", 1, nil, "rule 0", "stdout before", "end none")
				healthPlaySig(t, h, healthStdoutHold, func(k int) {
					at := fmt.Sprintf("phase %d", k)
					switch k {
					case 0:
						h.processed(t, at, "hs_calc", "CLEAR")
					case 1:
						h.compareNow(t, at+": the notifier's calls", transcript, healthCallsWant(1, healthCallsEnded(0)))
					case 2:
						h.compareNow(t, at+": the notifier's calls", transcript, healthCallsWant(2, written))
					case 3:
						h.compareNow(t, at+": the notifier's calls", transcript, healthCallsWant(3, written,
							healthCallFor("hs_calc", "CLEAR", 1, nil, "end exit 0")))
					}
				})
				// a death by SIGPIPE is a success to the agent (spawn_popen.c:144-162)
				h.compareNow(t, "the alert log's notification state", h.execView, healthHas(
					"hs_calc: WARNING->CRITICAL exec_run=T exec_code=0 exec_failed=false processed=true updated=true"))
			},
			after: records(false, healthRecordsWant(map[string]int{healthSent: 3, "killed by signal 13: ": 1,
				"SPAWN SERVER: process created with pid P: ": 3})),
		},
		"exit": {
			conf: healthCalcConf,
			sc:   healthValues("hsig.values", "hsig.ctx", []string{"a"}, values(10, 70, 95)...),
			ctl:  notify.Control{Rules: []notify.Rule{{Alert: "hs_calc", Status: "CRITICAL", SleepMs: 30000, IgnoreTerm: true}}},
			play: func(t *testing.T, h *healthPair) {
				transcript := h.transcript(t)
				healthPlayPhases(t, h, 3, healthCalcHold, func(k int) {
					at := fmt.Sprintf("phase %d", k)
					switch k {
					case 0:
						h.processed(t, at, "hs_calc", "CLEAR")
					case 1:
						h.compareNow(t, at+": the notifier's calls", transcript, healthCallsWant(1, healthCallsEnded(0)))
					case 2:
						// the call runs, and HEALTH waits for it
						h.compareNow(t, at+": the notifier's calls", transcript, healthCallsWant(2,
							healthCallFor("hs_calc", "CRITICAL", 1, nil, "rule 0", "end none")))
					}
				})
				// both stop well inside the call's 30 s, and inside HEALTH's wait for it
				time.Sleep(healthExitInto)
			},
			after: func(t *testing.T, h *healthPair) {
				// HEALTH ends its wait and kills the call: its record blames the timeout, and the spawn client, whose
				// waits end at once in a thread that was told to stop, sends SIGTERM and SIGKILL and waits for neither
				records(true, healthRecordsWant(map[string]int{
					"daemon.log ": 1, healthPastLimit: 1, "collector.log ": 1,
					"SPAWN PARENT: giving up waiting for pid P after SIGKILL (request R) - reclaiming, the child is left running: ": 1}))(t, h)
				// what the stop leaves of the entries: the killed call's code is nowhere, its row still says the call runs
				h.compareLines(t, "the alert log's rows", func(i int) []string {
					var rows []string
					for _, l := range h.n[i].healthDump(t, h.p.Each()[i].Daemon) {
						if strings.HasPrefix(l, "row health_log_detail ") {
							rows = append(rows, l)
						}
					}
					return rows
				}, func(oracle []string) error {
					// SAVED, PROCESSED, EXEC_RUN and EXEC_IN_PROGRESS (health/health.h:10-20), and no code
					if len(oracle) != 6 || !strings.Contains(oracle[5], " flags=268435525 ") || !strings.Contains(oracle[5], " exec_code=0 ") {
						return fmt.Errorf("want 6 rows, the last with flags 0x10000045 and no code")
					}
					return nil
				})
			},
		},
	})
}

const (
	// healthStdoutHold is how long the `stdout` case holds each value: HEALTH waits 3 s for the CRITICAL call, inside
	// its iteration, before it evaluates again
	healthStdoutHold = 7 * time.Second
	// healthExitInto is how long after the CRITICAL call was seen the `exit` case stops both agents
	healthExitInto = 2 * time.Second
)
