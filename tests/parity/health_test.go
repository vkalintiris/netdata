// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/json"
	"fmt"
	"io"
	"maps"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/notify"
	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The health checks' runner (M9 commit 0, D183; plan evidence/2026-10-03-plan-m9-commit0.md §5). Each case boots both
// agents with health on (`Options.HealthOn`), the stock alerts off, one user health.d file, the recording notifier
// (package notify) as `script to execute on alarm`, a health pass every second, a fixed management key and the fake
// plugin, whose one chart both plugins create at the same second once both agents are ready, and whose values the
// case switches at the middle of a second: both agents store the same series, second for second. Every comparison
// checks the oracle first: a guard names the health state it must have reached, so two agents without health never
// pass; then the candidate gets a bounded wait to show the same view.

const (
	// healthConfFile is a case's one file under <run>/etc/health.d (one file: no readdir order)
	healthConfFile = "parity.conf"
	// healthKey is the management key written into both run directories before the start (api_v1_manage.c:7-127)
	healthKey = "5a1e0000-0000-4000-8000-0000000c0de5"
	// the phase barrier's bounds: the oracle reaches its guarded state, then the candidate the oracle's view
	healthOracleWait    = 15 * time.Second
	healthCandidateWait = 10 * time.Second
)

// healthCase is one case of a health check.
type healthCase struct {
	// conf is the text of the user health.d file; empty: none
	conf string
	// extra are more [health] lines; stock keeps the stock health.d on; logs is a [logs] section; stream is appended
	// to stream.conf (a child's section)
	extra  string
	stock  bool
	logs   string
	stream string
	// ctl are the notifier's rules
	ctl notify.Control
	// sc is the fake plugin's scenario; nil: no chart (the plugin waits for the stop)
	sc *plugin.Scenario
	// play drives the case while both agents run and compares what is visible; after, when set, runs once both stopped
	// (the files and the logs)
	play  func(t *testing.T, h *healthPair)
	after func(t *testing.T, h *healthPair)
}

// healthPair is a case's two agents with what the helpers keep per side.
type healthPair struct {
	p  *Pair
	ls [2]plugin.Layout
	n  [2]*healthNorm
	// released is when the current phase began (the last release)
	released time.Time
}

// healthOptions are a case's options: the fake plugin's (one tier, ram children, pulse off), health on with the rails'
// notifier, a pass every second.
func healthOptions(c healthCase) daemon.Options {
	o := pluginsOptions(1, nil, nil, c.logs)
	o.HealthOn = true
	o.HealthExtra = "    script to execute on alarm = {run}/notify/stub\n    run at least every = 1s\n"
	if !c.stock {
		o.HealthExtra += "    enable stock health configuration = no\n"
	}
	o.HealthExtra += c.extra
	o.StreamExtra = c.stream
	return o
}

// healthIdle is the plugin of a case without a chart.
var healthIdle = plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Hang: true}}}}}

// healthValues is the fake plugin's scenario of a case with a chart: the chart is created at the `create` release,
// then each phase's values are collected until the release `p<n>` (n the next phase's index); the last phase holds.
func healthValues(chart, context string, dims []string, phases ...map[string]int64) *plugin.Scenario {
	v := &plugin.Values{Chart: chart, Context: context, Dims: dims}
	for i, set := range phases {
		v.Phases = append(v.Phases, plugin.Phase{Set: set, Until: fmt.Sprintf("p%d", i+1)})
	}
	return &plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{WaitFile: "create"}, {Values: v}}}}}
}

// healthPrepare lays out a side's run directory before its agent starts: the health.d file, the notifier with its
// rules and the management key.
func healthPrepare(t *testing.T, runDir string, c healthCase, stub string) {
	t.Helper()
	dir := filepath.Join(runDir, "etc", "health.d")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if c.conf != "" {
		if err := os.WriteFile(filepath.Join(dir, healthConfFile), []byte(c.conf), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if err := notify.Install(runDir, stub, c.ctl); err != nil {
		t.Fatal(err)
	}
	// the admin's bearer token: health's DynCfg nodes are not open to anonymous clients
	fnWriteTokens(t, runDir)
	if err := os.MkdirAll(filepath.Join(runDir, "lib"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(runDir, "lib", "netdata.api.key"), []byte(healthKey), 0o600); err != nil {
		t.Fatal(err)
	}
}

// runHealthCases plays each case on a pair with health on, then stops both and runs the case's comparisons of what
// they left.
func runHealthCases(t *testing.T, cases map[string]healthCase) {
	bins := binaries(t)
	engine, err := plugin.Engine()
	if err != nil {
		t.Fatal(err)
	}
	stub, err := notify.Stub()
	if err != nil {
		t.Fatal(err)
	}
	for _, name := range slices.Sorted(maps.Keys(cases)) {
		c := cases[name]
		t.Run(name, func(t *testing.T) {
			h := &healthPair{}
			sc := healthIdle
			if c.sc != nil {
				sc = *c.sc
			}
			side := 0
			h.p = startPairWith(t, healthOptions(c), parentIdentity, bins, [2]string{}, [2]Role{Oracle, Candidate},
				func(t *testing.T, runDir string) {
					healthPrepare(t, runDir, c, stub)
					l, err := plugin.Install(runDir, engine, sc)
					if err != nil {
						t.Fatal(err)
					}
					h.ls[side] = l
					side++
				})
			for i, s := range h.p.Each() {
				h.n[i] = newHealthNorm(s.Daemon)
			}
			h.rails(t)
			c.play(t, h)
			h.noRealNotifier(t)
			stopBoth(t, h.p)
			if c.after != nil {
				c.after(t, h)
			}
		})
	}
}

var healthScriptRe = regexp.MustCompile(`(?m)^\s*(# )?script to execute on alarm = (.*)$`)

// rails checks what every case stands on, before it plays: each agent's configuration names the side's stub as the
// notifier (the default is the installed alarm-notify.sh, which may send mail), and the oracle runs health.
func (h *healthPair) rails(t *testing.T) {
	t.Helper()
	for _, s := range h.p.Each() {
		r, err := Get(s.Daemon, "/netdata.conf", nil)
		if err != nil {
			t.Fatalf("%s: /netdata.conf: %v", s.Role, err)
		}
		m := healthScriptRe.FindSubmatch(r.Body)
		if m == nil || string(m[2]) != notify.Path(s.Daemon.Opts.RunDir) {
			t.Fatalf("%s: the notifier is not the stub: /netdata.conf has %q, want %q", s.Role, m, notify.Path(s.Daemon.Opts.RunDir))
		}
	}
	if a, status, _ := healthState(h.p.Oracle, "/api/v1/alarms"); !status {
		t.Fatalf("oracle: health is not running: /api/v1/alarms answered %d %q", a.Status, a.Body)
	}
	h.noRealNotifier(t)
}

// healthState reads the top of an /api/v1/alarms answer: whether the host's health runs, and the last unique id its
// alert log gave (health/health_json.c:278-291).
func healthState(d *daemon.Daemon, path string) (Response, bool, int64) {
	a := healthGet(d, path)
	var doc struct {
		Status bool  `json:"status"`
		Latest int64 `json:"latest_alarm_log_unique_id"`
	}
	if a.Status != http.StatusOK || json.Unmarshal(a.Body, &doc) != nil {
		return a, false, 0
	}
	return a, doc.Status, doc.Latest
}

// noRealNotifier fails the case when a process named alarm-notify.sh descends from either agent.
func (h *healthPair) noRealNotifier(t *testing.T) {
	t.Helper()
	cmdlines, _ := filepath.Glob("/proc/[0-9]*/cmdline")
	for _, f := range cmdlines {
		b, err := os.ReadFile(f)
		if err != nil || !strings.Contains(string(b), "alarm-notify.sh") {
			continue
		}
		pid, _ := strconv.Atoi(filepath.Base(filepath.Dir(f)))
		for p := pid; p > 1; p = healthParentOf(p) {
			for _, s := range h.p.Each() {
				if p == s.Daemon.PID() {
					t.Fatalf("%s: the installed notifier runs (pid %d): %q", s.Role, pid, strings.ReplaceAll(string(b), "\x00", " "))
				}
			}
		}
	}
}

// healthParentOf is a process's parent (0 when it is gone).
func healthParentOf(pid int) int {
	b, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid))
	if err != nil {
		return 0
	}
	// pid (comm) state ppid: the comm may hold spaces and parentheses
	s := string(b)
	i := strings.LastIndexByte(s, ')')
	f := strings.Fields(s[i+1:])
	if i < 0 || len(f) < 2 {
		return 0
	}
	ppid, _ := strconv.Atoi(f[1])
	return ppid
}

var healthClient = &http.Client{Timeout: 10 * time.Second}

// healthGet is a GET of path with header lines (`Name: value`); a transport error is a status 0 with its text.
func healthGet(d *daemon.Daemon, path string, headers ...string) Response {
	req, err := http.NewRequest(http.MethodGet, d.BaseURL+path, nil)
	if err != nil {
		return Response{Body: []byte(err.Error())}
	}
	for _, hl := range headers {
		k, v, _ := strings.Cut(hl, ": ")
		req.Header.Set(k, v)
	}
	resp, err := healthClient.Do(req)
	if err != nil {
		return Response{Body: []byte(err.Error())}
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return Response{Body: []byte(err.Error())}
	}
	return Response{Status: resp.StatusCode, ContentType: resp.Header.Get("Content-Type"), Body: body}
}

// healthView is a response as the checks compare it: the status, the content type, then the body as rendered.
func healthView(r Response, body string) string {
	return fmt.Sprintf("HTTP %d, %s\n%s", r.Status, r.ContentType, body)
}

// get is side i's view of a hand-built v1 answer about localhost (healthNorm.json).
func (h *healthPair) get(i int, path string, headers ...string) string {
	return h.getAs(h.n[i], i, path, headers...)
}

// getAs is get with another host's normalizer (a child's ids count from its own seeds).
func (h *healthPair) getAs(n *healthNorm, i int, path string, headers ...string) string {
	r := healthGet(h.p.Each()[i].Daemon, path, headers...)
	return healthView(r, n.json(string(r.Body)))
}

// plain is side i's view of an answer compared as it is, but the side's directories.
func (h *healthPair) plain(i int, path string, headers ...string) string {
	r := healthGet(h.p.Each()[i].Daemon, path, headers...)
	return healthView(r, h.n[i].paths(string(r.Body)))
}

// healthMidSecond sleeps to the middle of a wall-clock second.
func healthMidSecond() {
	now := time.Now()
	mid := now.Truncate(time.Second).Add(500 * time.Millisecond)
	if !mid.After(now) {
		mid = mid.Add(time.Second)
	}
	time.Sleep(time.Until(mid))
}

// release ends the fake plugins' current phase once it has lasted `hold`: both get `file` at the middle of a second,
// so both find it at the next whole second and both agents store the next phase's values from the same second on. It
// returns that second. The plugins' own records tell each side's: a difference, or a plugin that did not reach the
// phase, is the harness's failure, not a verdict on the agents.
func (h *healthPair) release(t *testing.T, file string, phase int, hold time.Duration) int64 {
	t.Helper()
	if wait := time.Until(h.released.Add(hold)); wait > 0 {
		time.Sleep(wait)
	}
	healthMidSecond()
	for i := range h.ls {
		if err := h.ls[i].Release(file); err != nil {
			t.Fatal(err)
		}
	}
	h.released = time.Now()
	var secs [2]int64
	for i, s := range h.p.Each() {
		key := strconv.Itoa(phase)
		starts, ok := h.ls[i].WaitFor(5*time.Second, func(starts [][]plugin.Record) bool {
			if len(starts) == 0 {
				return false
			}
			_, ok := plugin.PhaseSeconds(starts[len(starts)-1])[key]
			return ok
		})
		if !ok {
			t.Fatalf("harness: %s: the plugin did not reach phase %d within 5 s of %q (%d starts)", s.Role, phase, file, len(starts))
		}
		secs[i] = plugin.PhaseSeconds(starts[len(starts)-1])[key]
	}
	if secs[0] != secs[1] {
		t.Fatalf("harness: the sides switched to phase %d at different seconds: oracle %d, candidate %d", phase, secs[0], secs[1])
	}
	return secs[0]
}

// create has both plugins create their chart at the same second, two seconds after both agents are ready, waits for
// the alert logs' first entries (settle) and returns the chart's first second. C runs no health on a host whose
// database holds nothing yet (database/rrdhost.c:964-969, rrdhost-status.c:124-131): with every collector off,
// HEALTH's first pass on localhost comes once this chart has data, and links its alerts three times
// (health_event_loop.c:211-256: the host's pending label recheck).
func (h *healthPair) create(t *testing.T) int64 {
	t.Helper()
	time.Sleep(2 * time.Second)
	sec := h.release(t, "create", 0, 0)
	h.settle(t, h.n, "")
	return sec
}

// settle waits until a host's alert log (localhost's for an empty prefix, else `/host/<name>`) shows its first
// entries on each side, so the side's id bases are the log's seeds: the entries logged when alerts are linked are
// stored by the metadata thread a few seconds later, after the first evaluations' (health_log.c:59-76). The log is
// settled when its entries run without a hole from its lowest id to the last id the host gave
// (`latest_alarm_log_unique_id`) and the lowest is an alert's first event. The oracle must get there (the case fails
// as `oracle: …`); the candidate gets the bounded wait and is then compared as it is.
func (h *healthPair) settle(t *testing.T, n [2]*healthNorm, prefix string) {
	t.Helper()
	settled := func(i int) error {
		d := h.p.Each()[i].Daemon
		_, _, latest := healthState(d, prefix+"/api/v1/alarms")
		entries, err := h.entriesAs(n[i], i, prefix+"/api/v1/alarm_log")
		if err != nil {
			return err
		}
		if len(entries) == 0 || entries[0].EventID != 1 || entries[len(entries)-1].UniqueID != latest ||
			int64(len(entries)) != latest-entries[0].UniqueID+1 {
			return fmt.Errorf("%d entries, the last id given %d", len(entries), latest)
		}
		return nil
	}
	h.waitOracle(t, "the alert log's first entries"+prefix, func() (string, error) { return "", settled(0) })
	for end := time.Now().Add(healthCandidateWait); settled(1) != nil && time.Now().Before(end); time.Sleep(250 * time.Millisecond) {
		// a candidate that does not serve the log is compared at once
		if r := healthGet(h.p.Candidate, prefix+"/api/v1/alarm_log"); r.Status != http.StatusOK {
			break
		}
	}
}

// compareNow compares a view of both sides. The oracle's must satisfy `guard` within healthOracleWait (the case fails
// as `oracle: …` otherwise: the health state the comparison stands on was not reached); then the candidate has
// healthCandidateWait to show the same view, and the first difference left ends the case, naming what was compared
// with both views. While the candidate differs the oracle's view is taken again (a value still settling), and kept
// only under its guard.
func (h *healthPair) compareNow(t *testing.T, what string, view func(i int) string, guard func(oracle string) error) string {
	t.Helper()
	oracle := h.waitOracle(t, what, func() (string, error) {
		o := view(0)
		return o, guard(o)
	})
	end := time.Now().Add(healthCandidateWait)
	for {
		candidate := view(1)
		if candidate != oracle {
			if again := view(0); guard(again) == nil {
				oracle = again
			}
		}
		if candidate == oracle {
			t.Logf("%s, both sides:\n%s", what, healthBrief(oracle))
			return oracle
		}
		if time.Now().After(end) {
			o, c := healthUnlike(oracle, candidate)
			t.Fatalf("%s differs after %v\noracle:\n%s\ncandidate:\n%s", what, healthCandidateWait, o, c)
		}
		time.Sleep(250 * time.Millisecond)
	}
}

// healthBrief is a view as a passing comparison logs it: a notifier transcript's environment lines (the agent's whole
// environment, compared but long) are left out, counted.
func healthBrief(view string) string {
	var out []string
	env := 0
	for _, l := range strings.Split(view, "\n") {
		if healthEnvLineRe.MatchString(l) {
			env++
			continue
		}
		out = append(out, l)
	}
	if env > 0 {
		out = append(out, fmt.Sprintf("(and %d environment lines)", env))
	}
	return strings.Join(out, "\n")
}

var healthEnvLineRe = regexp.MustCompile(`^call \d+: env `)

// healthUnlike is two differing views as a failure prints them: whole, but the environment lines both hold.
func healthUnlike(oracle, candidate string) (string, string) {
	strip := func(view, other string) string {
		held := map[string]bool{}
		for _, l := range strings.Split(other, "\n") {
			held[l] = true
		}
		var out []string
		for _, l := range strings.Split(view, "\n") {
			if !healthEnvLineRe.MatchString(l) || !held[l] {
				out = append(out, l)
			}
		}
		return strings.Join(out, "\n")
	}
	return strip(oracle, candidate), strip(candidate, oracle)
}

// waitOracle polls the oracle until `state` reports no error, and returns what it read; the case fails as
// `oracle: …` after healthOracleWait.
func (h *healthPair) waitOracle(t *testing.T, what string, state func() (string, error)) string {
	t.Helper()
	end := time.Now().Add(healthOracleWait)
	for {
		got, err := state()
		if err == nil {
			return got
		}
		if time.Now().After(end) {
			t.Fatalf("oracle: %s: %v within %v:\n%s", what, err, healthOracleWait, got)
		}
		time.Sleep(250 * time.Millisecond)
	}
}

// waitCandidate gives the candidate the bounded wait to show the oracle's view of path, without a verdict: a case
// whose comparison is of what the agents leave after the stop must not stop a candidate one pass behind the oracle.
// A candidate that does not answer path with 200 is not waited for.
func (h *healthPair) waitCandidate(path string, view func(i int) string) {
	for end := time.Now().Add(healthCandidateWait); time.Now().Before(end); time.Sleep(250 * time.Millisecond) {
		if r := healthGet(h.p.Candidate, path); r.Status != http.StatusOK || view(1) == view(0) {
			return
		}
	}
}

// compareLines compares what both sides left (files, logs, transcripts) once the oracle's lines passed their guard.
func (h *healthPair) compareLines(t *testing.T, what string, lines func(i int) []string, guard func(oracle []string) error) {
	t.Helper()
	oracle := lines(0)
	if err := guard(oracle); err != nil {
		t.Fatalf("oracle: %s: %v:\n%s", what, err, strings.Join(oracle, "\n"))
	}
	candidate := lines(1)
	if !slices.Equal(oracle, candidate) {
		o, c := healthUnlike(strings.Join(oracle, "\n"), strings.Join(candidate, "\n"))
		t.Fatalf("%s differ\noracle:\n%s\ncandidate:\n%s", what, o, c)
	}
	t.Logf("%s, both sides:\n%s", what, healthBrief(strings.Join(oracle, "\n")))
}

// healthEntry is an entry of /api/v1/alarm_log, as the guards and the per-alert views read it.
type healthEntry struct {
	UniqueID   int64  `json:"unique_id"`
	EventID    int64  `json:"alarm_event_id"`
	Name       string `json:"name"`
	Status     string `json:"status"`
	OldStatus  string `json:"old_status"`
	Value      string `json:"value_string"`
	ExecRun    int64  `json:"exec_run"`
	ExecCode   int64  `json:"exec_code"`
	ExecFailed bool   `json:"exec_failed"`
	Processed  bool   `json:"processed"`
	Updated    bool   `json:"updated"`
}

// healthBody is a view's body (after the status line).
func healthBody(view string) string {
	_, body, _ := strings.Cut(view, "\n")
	return body
}

// healthStatuses reads a view of /api/v1/alarms?all: each alert's status by name.
func healthStatuses(view string) (map[string]string, error) {
	if !strings.HasPrefix(view, "HTTP 200, ") {
		return nil, fmt.Errorf("answered %q", strings.SplitN(view, "\n", 2)[0])
	}
	// the view's ids and times are no numbers any more: only the members the guards read are taken
	out := map[string]string{}
	for _, m := range healthAlarmStatusRe.FindAllStringSubmatch(healthBody(view), -1) {
		out[m[1]] = m[2]
	}
	return out, nil
}

// an alert's name, then (after the members between) its status (health/health_json.c:53-70)
var healthAlarmStatusRe = regexp.MustCompile(`(?s)"name": "([^"]*)",.*?"status": "([A-Z]+)"`)

// an alert's name, then its disabled and silenced flags (health_json.c:56-64)
var healthAlarmFlagsRe = regexp.MustCompile(`(?s)"name": "([^"]*)",.*?"disabled": (true|false),\s*"silenced": (true|false)`)

// healthWant is a guard on a view of /api/v1/alarms?all: each named alert has its status, and no other alert shows.
func healthWant(want map[string]string) func(string) error {
	return func(view string) error {
		got, err := healthStatuses(view)
		if err != nil {
			return err
		}
		if !maps.Equal(got, want) {
			return fmt.Errorf("the alerts are %v, want %v", got, want)
		}
		return nil
	}
}

// entries reads side i's alert log of localhost (/api/v1/alarm_log), oldest first.
func (h *healthPair) entries(i int, query string) ([]healthEntry, error) {
	return h.entriesAs(h.n[i], i, "/api/v1/alarm_log"+query)
}

// entriesAs reads an alert log at path (a child's: `/host/<name>/api/v1/alarm_log`) with its host's normalizer.
func (h *healthPair) entriesAs(n *healthNorm, i int, path string) ([]healthEntry, error) {
	r := healthGet(h.p.Each()[i].Daemon, path)
	if r.Status != http.StatusOK {
		return nil, fmt.Errorf("%s answered %d %q", path, r.Status, r.Body)
	}
	n.observe(string(r.Body))
	var out []healthEntry
	if err := json.Unmarshal(r.Body, &out); err != nil {
		return nil, fmt.Errorf("%s: %v: %q", path, err, r.Body)
	}
	slices.SortFunc(out, func(a, b healthEntry) int { return int(a.UniqueID - b.UniqueID) })
	return out, nil
}

// transitions is side i's alert log as per-alert transition lists: for each alert, in name order, its entries in
// log order as `name: OLD->NEW value`. A failed read is its text (the candidate's, compared against the oracle's
// lists).
func (h *healthPair) transitions(i int, query string) string {
	return h.transitionsAs(h.n[i], i, "/api/v1/alarm_log"+query)
}

// transitionsAs is transitions of the alert log at path, with its host's normalizer.
func (h *healthPair) transitionsAs(n *healthNorm, i int, path string) string {
	entries, err := h.entriesAs(n, i, path)
	if err != nil {
		return err.Error()
	}
	return strings.Join(healthTransitions(entries), "\n")
}

func healthTransitions(entries []healthEntry) []string {
	by := map[string][]string{}
	for _, e := range entries {
		by[e.Name] = append(by[e.Name], fmt.Sprintf("%s: %s->%s %s", e.Name, e.OldStatus, e.Status, e.Value))
	}
	var out []string
	for _, name := range slices.Sorted(maps.Keys(by)) {
		out = append(out, by[name]...)
	}
	return out
}

// healthSequence is an alert's statuses in a transition list: its first entry's old status, then each entry's new.
func healthSequence(lines []string, name string) []string {
	var out []string
	for _, l := range lines {
		rest, ok := strings.CutPrefix(l, name+": ")
		if !ok {
			continue
		}
		change, _, _ := strings.Cut(rest, " ")
		from, to, _ := strings.Cut(change, "->")
		if len(out) == 0 {
			out = append(out, from)
		}
		out = append(out, to)
	}
	return out
}
