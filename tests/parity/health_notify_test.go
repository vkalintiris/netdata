// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"regexp"
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

// healthExecView is side i's alert log as each entry's notification state: `name: OLD->NEW exec_run=T|0 exec_code=N
// exec_failed=… processed=… updated=…`, oldest first.
func (h *healthPair) execView(i int) string {
	entries, err := h.entries(i, "")
	if err != nil {
		return err.Error()
	}
	var out []string
	for _, e := range entries {
		run := "0"
		if e.ExecRun != 0 {
			run = "T"
		}
		out = append(out, fmt.Sprintf("%s: %s->%s exec_run=%s exec_code=%d exec_failed=%t processed=%t updated=%t", e.Name,
			e.OldStatus, e.Status, run, e.ExecCode, e.ExecFailed, e.Processed, e.Updated))
	}
	return strings.Join(out, "\n")
}

var (
	// HEALTH's records about a notification (health_notifications.c:24-82) and the spawn server's about the stub
	healthNotifyRecordRe = regexp.MustCompile(`msg="HEALTH: alert notification`)
	healthStubRecordRe   = regexp.MustCompile(`notify/stub`)
)

// healthNotifyRecords are a side's records about its notifications: HEALTH's in daemon.log, then every collector.log
// record naming the stub (the spawn server's), pids and request numbers masked, the command's arguments rendered as
// the transcripts' (healthNorm.command).
func healthNotifyRecords(t *testing.T, n *healthNorm, d *daemon.Daemon) []string {
	t.Helper()
	var out []string
	add := func(file, l string) {
		l = n.command(normalizeLog(l, d.Opts.RunDir, ""))
		l = pidRe.ReplaceAllString(l, "pid P")
		out = append(out, file+" "+requestRe.ReplaceAllString(l, "request R"))
	}
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if healthNotifyRecordRe.MatchString(l) {
			add("daemon.log", l)
		}
	}
	for _, l := range logLines(t, d.Opts.RunDir, "collector.log") {
		if healthStubRecordRe.MatchString(l) {
			add("collector.log", l)
		}
	}
	return out
}

// TestHealthNotify (check `health.notify`, M9 commit 0, D183): one alert through CLEAR, WARNING, CRITICAL and CLEAR
// with the recording notifier. At each phase the notifier's transcripts are compared (every argument, the
// environment, the directory, the parent, how each call ended: healthNorm.call), then the alert log's notification
// state and, both stopped, the agents' records about the notifications. `fail` has the CRITICAL call exit 3; `timeout`
// has it sleep past a 2 s execution timeout ignoring SIGTERM, so the agent kills it.
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
			transcript := func(i int) string {
				return strings.Join(h.n[i].calls(t, h.p.Each()[i].Daemon), "\n")
			}
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
	after := func(want ...string) func(t *testing.T, h *healthPair) {
		return func(t *testing.T, h *healthPair) {
			h.compareLines(t, "the records about the notifications",
				func(i int) []string { return healthNotifyRecords(t, h.n[i], h.p.Each()[i].Daemon) },
				func(oracle []string) error {
					for _, w := range want {
						if !strings.Contains(strings.Join(oracle, "\n"), w) {
							return fmt.Errorf("no record holds %q", w)
						}
					}
					return nil
				})
		}
	}
	has := func(parts ...string) func(string) error {
		return func(view string) error {
			for _, p := range parts {
				if !strings.Contains(view, p) {
					return fmt.Errorf("no entry holds %q", p)
				}
			}
			return nil
		}
	}
	sc := healthValues("hsig.values", "hsig.ctx", []string{"a"}, healthSigPhases...)
	runHealthCases(t, map[string]healthCase{
		"basic": {
			conf: healthCalcConf, sc: sc,
			play: play("call 3: end exit 0", has("hs_calc: CLEAR->WARNING exec_run=T exec_code=0 exec_failed=false",
				"hs_calc: WARNING->CRITICAL exec_run=T exec_code=0 exec_failed=false")),
			after: after(),
		},
		"fail": {
			conf: healthCalcConf, sc: sc,
			ctl:   notify.Control{Rules: []notify.Rule{{Alert: "hs_calc", Status: "CRITICAL", Exit: 3}}},
			play:  play("call 2: end exit 3", has("hs_calc: WARNING->CRITICAL exec_run=T exec_code=3 exec_failed=true")),
			after: after("exited with exit code 3"),
		},
		"timeout": {
			conf: healthCalcConf, sc: sc,
			extra: "    notification execution timeout = 2s\n",
			ctl:   notify.Control{Rules: []notify.Rule{{Alert: "hs_calc", Status: "CRITICAL", SleepMs: 30000, IgnoreTerm: true}}},
			play:  play("call 2: end none", has("hs_calc: WARNING->CRITICAL exec_run=T exec_code=128 exec_failed=true")),
			after: after("is still running past its execution timeout - killing it"),
		},
	})
}
