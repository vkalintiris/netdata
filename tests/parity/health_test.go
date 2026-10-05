// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"cmp"
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
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/notify"
	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The health checks' runner (M9 commit 0, D183; plan evidence/2026-10-03-plan-m9-commit0.md §5). Each case boots both
// agents with health on (`Options.HealthOn`; healthCase.off: off, for what C serves without health), the stock alerts
// off, one user health.d file (a case may lay out more files and a stock tree of its own: healthCase.files, stockDir),
// the recording notifier (package notify) as `script to execute on alarm`, a health pass every second, a fixed
// management key (or the state of its file a case names: healthCase.key) and the fake plugin, whose charts both
// plugins create at the same second once both agents are ready
// (one collected chart, and before it the charts a case defines with lines of its own: healthScenario), and whose
// values the case switches at the middle of a second: both agents store the same series, second for second. Every
// comparison checks the oracle first: a guard names the health state it must have reached, so two agents without
// health never pass; then the candidate gets a bounded wait to show the same view. The views mask each side's clock
// and id seeds (health_norm_test.go); beside the masks, two equal views must hold their events' times, and the
// seconds a view of the variables' endpoints masked, within healthBound of each other (near), and the ids' seeds are
// bound to the chart's first second (bases). A candidate that serves no alert log has its ids read by its alarm ids'
// base (settleNoLog; D198 F1). While a case runs a watch samples /proc: the installed notifier must never run
// (healthWatch). A hand-back case (healthCase.again; D205 F1) stops both agents and starts each side's run directory a
// second time, with the binary the case names for the side: what an agent does with an alert log it finds.

const (
	// healthConfFile is a case's one file under <run>/etc/health.d (one file: no readdir order)
	healthConfFile = "parity.conf"
	// healthLink starts a healthCase.files value that makes a symbolic link: the rest is its target
	healthLink = "-> "
	// healthUnreadable is a healthCase.files value: a file nobody may open (mode 0; the agents do not run as root)
	healthUnreadable = "(unreadable)"
	// healthKey is the management key written into both run directories before the start (api_v1_manage.c:7-127),
	// unless the case names another state of the key's file (healthCase.key)
	healthKey = "5a1e0000-0000-4000-8000-0000000c0de5"
	// healthKeyNone is a healthCase.key value: no key file is laid out, so each agent makes a key of its own
	healthKeyNone = "(none)"
	// healthKeyDir is a healthCase.key value: a directory stands where the key's file would
	healthKeyDir = "(directory)"
	// healthKeyFile is the key's file under a run directory (`<varlib>/netdata.api.key`), and healthSilencersFile the
	// silencers' (`<varlib>/health.silencers.json`), unless a case names another (healthCase.keyFile, silencers)
	healthKeyFile       = "lib/netdata.api.key"
	healthSilencersFile = "lib/health.silencers.json"
	// the phase barrier's bounds: the oracle reaches its guarded state, then the candidate the oracle's view
	healthOracleWait    = 15 * time.Second
	healthCandidateWait = 10 * time.Second
	// healthBound is how far apart, in seconds, two agents may hold the time of one event or the seeds of their ids
	// (near, bases): C against C they were at most one second apart (R75: 203 `when`, 307 `delay_up_to_timestamp`,
	// 104 `last_status_change`, 74 `duration`, 62 `exec_run` members and the id bases of 24 cases)
	healthBound = 2
	// healthGridSecond is the second of an aligned window at which a case with a grid (healthCase.grid) creates its
	// chart and switches its values: the third of five, so the window of five seconds that holds a switch holds two
	// old values and three new ones. For the transition cases' values (10, 70, 95, 10) its average is 46, 85, 44:
	// CLEAR, WARNING, CLEAR, never a third status, so each switch shows as one transition. And a chart created there
	// has a whole window's end behind its first evaluation, whichever second's sample the pass finds stored
	// (created at the window's first second, the first value depends on that: R75 F10).
	healthGridSecond = 3
)

// healthCase is one case of a health check.
type healthCase struct {
	// conf is the text of the user health.d file; empty: none. `{run}` in it is each side's run directory (a rule's
	// `exec` under the side's notifier directory)
	conf string
	// files are more files, by path under the run directory: the value is the file's content, healthLink and a target
	// (relative to the link's directory) for a symbolic link, or healthUnreadable. They are made in path order, the same
	// on both sides.
	files map[string]string
	// stockDir makes <run>/stock the stock configuration directory, so <run>/stock/health.d is the case's stock tree
	// (with `stock`: else no stock rule is read)
	stockDir bool
	// extra are more [health] lines; stock keeps the stock health.d on; stream is appended to stream.conf (a child's
	// section)
	extra  string
	stock  bool
	stream string
	// off runs the case with health off: what C serves without it (the variables' endpoints, the chart's variables).
	// The rails then want the oracle's health off.
	off bool
	// hostLabels are the lines of netdata.conf's [host labels] section; logs those of its [logs] section
	// (healthLogsDebug: the records C writes at debug level, the link entries' and HEALTH's own)
	hostLabels string
	logs       string
	// grid, when set, is the length in seconds of the case's aligned lookup window: the chart is created and each
	// value switched at the window's healthGridSecond, so the windows hold the same mix of values in every run
	grid int64
	// key is the state of the management key's file before the start (api_v1_manage.c:7-123): empty, the fixed key
	// (healthKey) on both sides; healthKeyNone, no file; healthKeyDir, a directory in its place; healthLink and a
	// target, a symbolic link; any other text, a file that holds it. keyMode is the file's mode (0: 0600). keyFile,
	// when set, is the file's path under the run directory, which netdata.conf then names (`[registry] netdata
	// management api key file`); empty: C's default (healthKeyFile).
	key     string
	keyMode os.FileMode
	keyFile string
	// silencers, when set, is the silencers file's path under the run directory, which netdata.conf then names
	// (`[health] silencers file`); empty: C's default (healthSilencersFile)
	silencers string
	// ctl are the notifier's rules
	ctl notify.Control
	// sc is the fake plugin's scenario; nil: no chart (the plugin waits for the stop)
	sc *plugin.Scenario
	// dbMode is localhost's `[db] db`; empty: dbengine. A hand-back case runs in `alloc`: the host's database is then
	// empty at every start, so HEALTH's first pass waits for the chart's data in the second run as in the first
	// (healthPair.create), on both sides at the same second.
	dbMode string
	// play drives the case while both agents run and compares what is visible; after, when set, runs once both stopped
	// (the files and the logs)
	play  func(t *testing.T, h *healthPair)
	after func(t *testing.T, h *healthPair)
	// again, when set, makes the case a hand-back: once play ended both agents stop, each side's run directory is
	// started again (the same directories, so the same paths in the rules' sources and in the rows) and `again` drives
	// the second run; `after` then runs once that one stopped. bins names each side's binary in the first run and in the
	// second; a zero value is the oracle's and the candidate's.
	again func(t *testing.T, h *healthPair)
	bins  [2][2]Role
}

// healthPair is a case's two agents with what the helpers keep per side.
type healthPair struct {
	c  healthCase
	p  *Pair
	ls [2]plugin.Layout
	n  [2]*healthNorm
	// released is when the current phase began (the last release)
	released time.Time
	// raw are each side's answers by the view they were rendered as (near reads their event times)
	raw [2]map[string]string
	// clocks are, by view, the seconds the view's clock masks replaced (near bounds them side to side)
	clocks [2]map[string][]int64
	// watch looks for the installed notifier while the case runs
	watch *healthWatch
	// run counts the case's runs: 1, then 2 once a hand-back case started its agents again. The fake plugin's start of
	// that number is the one a release waits on.
	run int
	// keys are the management keys the sides' requests carry (healthPair.token): each side's own
	keys [2]string
	// saved counts the requests of the management API a case's oracle saved its silencers after, for the guard on
	// the records they left
	saved int
}

// healthOptions are a case's options: the fake plugin's (one tier, ram children, pulse off), health on (off for a case
// that says so) with the rails' notifier, a pass every second.
func healthOptions(c healthCase) daemon.Options {
	o := pluginsOptions(1, nil, nil, c.logs)
	o.HealthOn = !c.off
	o.DBMode = c.dbMode
	o.HostLabels = c.hostLabels
	o.HealthExtra = "    script to execute on alarm = {run}/notify/stub\n    run at least every = 1s\n"
	if !c.stock {
		o.HealthExtra += "    enable stock health configuration = no\n"
	}
	if c.silencers != "" {
		o.HealthExtra += "    silencers file = {run}/" + c.silencers + "\n"
	}
	o.HealthExtra += c.extra
	if c.keyFile != "" {
		// a second [registry] section: C adds its keys to the first one's
		o.ConfExtra += "\n[registry]\n    netdata management api key file = {run}/" + c.keyFile + "\n"
	}
	o.StreamExtra = c.stream
	if c.stockDir {
		o.StockConfigDir = "{run}/stock"
	}
	return o
}

// healthLogsDebug is a healthCase.logs value: every record down to debug level.
const healthLogsDebug = "    level = debug\n"

// healthIdle is the plugin of a case without a chart.
var healthIdle = plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Hang: true}}}}}

// healthValues is the fake plugin's scenario of a case with a chart: the chart is created at the `create` release,
// then each phase's values are collected until the release `p<n>` (n the next phase's index); the last phase holds.
func healthValues(chart, context string, dims []string, phases ...map[string]int64) *plugin.Scenario {
	return healthScenario("", chart, context, dims, phases...)
}

// healthScenario is healthValues with lines of the case's own, written in one piece at the `create` release, before
// the collected chart's definition: more charts (a name that is not the id, labels, a module), which exist and are
// never collected, and VARIABLE lines. The agent reads the lines in the order they were written, so both sides create
// the charts in one order: the lines' charts, then the collected one.
func healthScenario(emit, chart, context string, dims []string, phases ...map[string]int64) *plugin.Scenario {
	v := &plugin.Values{Chart: chart, Context: context, Dims: dims}
	for i, set := range phases {
		v.Phases = append(v.Phases, plugin.Phase{Set: set, Until: fmt.Sprintf("p%d", i+1)})
	}
	steps := []plugin.Step{{WaitFile: "create"}}
	if emit != "" {
		steps = append(steps, plugin.Step{Emit: emit})
	}
	return &plugin.Scenario{Starts: []plugin.Start{{Steps: append(steps, plugin.Step{Values: v})}}}
}

// healthPrepare lays out a side's run directory before its agent starts: the health.d file (`{run}` in it is the
// side's run directory), the case's other files, the notifier with its rules and the management key's file (the
// fixed key, or the state the case names: healthMakeKey). A rule that
// names a notifier of its own (`exec`) is refused unless it stays under the side's notifier directory or names a
// path nothing can be executed under: the rails on netdata.conf do not see a rule's line.
func healthPrepare(t *testing.T, runDir string, c healthCase, stub string) {
	t.Helper()
	dir := filepath.Join(runDir, "etc", "health.d")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	conf := strings.ReplaceAll(c.conf, "{run}", runDir)
	for _, text := range append([]string{conf}, slices.Collect(maps.Values(c.files))...) {
		if err := daemon.ValidateHealthRules(text, notify.Dir(runDir)+"/", "/dev/null/"); err != nil {
			t.Fatal(err)
		}
	}
	if conf != "" {
		if err := os.WriteFile(filepath.Join(dir, healthConfFile), []byte(conf), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if c.stockDir {
		// the agents refuse to start without their stock configuration directory
		if err := os.MkdirAll(filepath.Join(runDir, "stock", "health.d"), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	for _, path := range slices.Sorted(maps.Keys(c.files)) {
		if err := healthMakeFile(filepath.Join(runDir, path), c.files[path]); err != nil {
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
	if err := healthMakeKey(filepath.Join(runDir, cmp.Or(c.keyFile, healthKeyFile)), c.key, c.keyMode); err != nil {
		t.Fatal(err)
	}
}

// healthMakeKey lays out the management key's file as a case wants it before the start (healthCase.key): the fixed
// key, nothing, a directory, a symbolic link (its target relative to the file's directory), or a file that holds the
// text, with the mode asked for whatever the umask.
func healthMakeKey(file, state string, mode os.FileMode) error {
	if err := os.MkdirAll(filepath.Dir(file), 0o755); err != nil {
		return err
	}
	switch target, link := strings.CutPrefix(state, healthLink); {
	case state == healthKeyNone:
		return nil
	case state == healthKeyDir:
		return os.Mkdir(file, 0o755)
	case link:
		return os.Symlink(target, file)
	}
	if err := os.WriteFile(file, []byte(cmp.Or(state, healthKey)), 0o600); err != nil {
		return err
	}
	return os.Chmod(file, cmp.Or(mode, 0o600))
}

// key is what side i's key file holds now (the fixed key, the text a case laid out, or the key the agent made at its
// start); empty when nothing can be read there.
func (h *healthPair) key(i int) string {
	b, err := os.ReadFile(filepath.Join(h.p.Each()[i].Daemon.Opts.RunDir, cmp.Or(h.c.keyFile, healthKeyFile)))
	if err != nil {
		return ""
	}
	return string(b)
}

// token is the management key side i's requests carry: the one the case set for the side (healthPair.keys: a key
// that is not in the file), else what the side's key file held when it was first asked for. With the fixed key both
// sides carry the same one; without it each side carries its own.
func (h *healthPair) token(i int) string {
	if h.keys[i] == "" {
		h.keys[i] = h.key(i)
	}
	return h.keys[i]
}

// healthMakeFile makes one of a case's files (healthCase.files) with its directories.
func healthMakeFile(file, content string) error {
	if err := os.MkdirAll(filepath.Dir(file), 0o755); err != nil {
		return err
	}
	if target, link := strings.CutPrefix(content, healthLink); link {
		return os.Symlink(target, file)
	}
	if content == healthUnreadable {
		return os.WriteFile(file, nil, 0)
	}
	return os.WriteFile(file, []byte(content), 0o644)
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
			h := &healthPair{c: c, raw: [2]map[string]string{{}, {}}, clocks: [2]map[string][]int64{{}, {}},
				watch: watchRealNotifier()}
			// a case that ends early still says what the watch saw
			t.Cleanup(func() {
				h.watch.end()
				if seen := h.watch.sightings(); len(seen) > 0 {
					t.Errorf("the installed notifier ran under an agent:\n%s", strings.Join(seen, "\n"))
				}
			})
			sc := healthIdle
			if c.sc != nil {
				sc = *c.sc
			}
			// each side's binary, per run: the oracle's and the candidate's unless the case says otherwise
			binOf := func(run, i int) string {
				if c.bins[run][i] == Oracle || (c.bins[run][i] == "" && i == 0) {
					return bins[0]
				}
				return bins[1]
			}
			side := 0
			h.run = 1
			h.p = startPairWith(t, healthOptions(c), parentIdentity, [2]string{binOf(0, 0), binOf(0, 1)}, [2]string{},
				[2]Role{Oracle, Candidate}, func(t *testing.T, runDir string) {
					healthPrepare(t, runDir, c, stub)
					l, err := plugin.Install(runDir, engine, sc)
					if err != nil {
						t.Fatal(err)
					}
					h.ls[side] = l
					side++
				})
			for i, s := range h.p.Each() {
				h.n[i] = newHealthNorm(s.Daemon, "")
			}
			h.rails(t)
			c.play(t, h)
			stopBoth(t, h.p)
			if c.again != nil {
				h.noRealNotifier(t)
				h.restart(t, [2]string{binOf(1, 0), binOf(1, 1)})
				h.rails(t)
				c.again(t, h)
				stopBoth(t, h.p)
			}
			h.watch.end()
			h.noRealNotifier(t)
			if c.after != nil {
				c.after(t, h)
			}
		})
	}
}

// restart starts both sides again, each in its own run directory with the binary given (a hand-back case's second
// run: the configuration, the cache and the fake plugin's directory are the first run's), both at once, so the two
// second runs begin within a start-up of each other. The fake plugin plays its next start, and each side's
// normalizer goes on: the second run's ids print by the first run's bases, so ids that do not continue show.
func (h *healthPair) restart(t *testing.T, bins [2]string) {
	t.Helper()
	var wg sync.WaitGroup
	var errs [2]error
	for i, s := range h.p.Each() {
		wg.Add(1)
		go func() {
			defer wg.Done()
			s.Daemon.Opts.Binary = bins[i]
			errs[i] = s.Daemon.Restart()
		}()
	}
	wg.Wait()
	for i, s := range h.p.Each() {
		if errs[i] != nil {
			t.Fatalf("parity: start %s again (%s): %v", s.Role, bins[i], errs[i])
		}
	}
	h.run++
	h.released = time.Time{}
}

// the notifier in a /netdata.conf dump: a commented line would be the default, which the stub never is
var healthScriptRe = regexp.MustCompile(`(?m)^\s*script to execute on alarm = (.*)$`)

// rails checks what every case stands on, before it plays: each agent's configuration names the side's stub as the
// notifier (the default is the installed alarm-notify.sh, which may send mail), and the oracle runs health (a case
// with health off: the oracle answers, and says its health is off).
func (h *healthPair) rails(t *testing.T) {
	t.Helper()
	for _, s := range h.p.Each() {
		r, err := Get(s.Daemon, "/netdata.conf", nil)
		if err != nil {
			t.Fatalf("%s: /netdata.conf: %v", s.Role, err)
		}
		m := healthScriptRe.FindSubmatch(r.Body)
		if m == nil || string(m[1]) != notify.Path(s.Daemon.Opts.RunDir) {
			t.Fatalf("%s: the notifier is not the stub: /netdata.conf has %q, want %q", s.Role, m, notify.Path(s.Daemon.Opts.RunDir))
		}
	}
	switch a, status, _ := healthState(h.p.Oracle, "/api/v1/alarms"); {
	case h.c.off && (status || !healthOffRe.Match(a.Body)):
		t.Fatalf("oracle: health is not off: /api/v1/alarms answered %d %q", a.Status, a.Body)
	case !h.c.off && !status:
		t.Fatalf("oracle: health is not running: /api/v1/alarms answered %d %q", a.Status, a.Body)
	}
	h.noRealNotifier(t)
}

// the top of an /api/v1/alarms answer of a host whose health is off (health/health_json.c:278-291)
var healthOffRe = regexp.MustCompile(`^\{\n\t"hostname": "[^"]*",\n\t"latest_alarm_log_unique_id": 0,\n\t"status": false,`)

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

// healthWatch samples /proc for the length of a case: the installed notifier (alarm-notify.sh, which may send mail)
// must never run under an agent, whatever its configuration says. Both agents are this process's children.
type healthWatch struct {
	stop, done chan struct{}
	once       sync.Once
	mu         sync.Mutex
	seen       []string
}

// watchRealNotifier starts a case's watch: every 100 ms, each process whose command line names alarm-notify.sh and
// that descends from this process is kept with its pid and its command line.
func watchRealNotifier() *healthWatch {
	w := &healthWatch{stop: make(chan struct{}), done: make(chan struct{})}
	go func() {
		defer close(w.done)
		for {
			w.sample()
			select {
			case <-w.stop:
				return
			case <-time.After(100 * time.Millisecond):
			}
		}
	}()
	return w
}

func (w *healthWatch) sample() {
	self := os.Getpid()
	cmdlines, _ := filepath.Glob("/proc/[0-9]*/cmdline")
	for _, f := range cmdlines {
		b, err := os.ReadFile(f)
		if err != nil || !strings.Contains(string(b), "alarm-notify.sh") {
			continue
		}
		pid, _ := strconv.Atoi(filepath.Base(filepath.Dir(f)))
		for p := pid; p > 1; p, _, _ = procStat(p) {
			if p == self {
				w.mu.Lock()
				w.seen = append(w.seen, fmt.Sprintf("pid %d: %q", pid, strings.ReplaceAll(string(b), "\x00", " ")))
				w.mu.Unlock()
				break
			}
		}
	}
}

// end stops the watch after a last sample.
func (w *healthWatch) end() {
	w.once.Do(func() {
		close(w.stop)
		<-w.done
		w.sample()
	})
}

// sightings are the processes the watch saw so far.
func (w *healthWatch) sightings() []string {
	w.mu.Lock()
	defer w.mu.Unlock()
	return slices.Clone(w.seen)
}

// noRealNotifier fails the case when the watch saw the installed notifier run under an agent.
func (h *healthPair) noRealNotifier(t *testing.T) {
	t.Helper()
	if seen := h.watch.sightings(); len(seen) > 0 {
		t.Fatalf("the installed notifier ran under an agent:\n%s", strings.Join(seen, "\n"))
	}
}

var healthClient = &http.Client{Timeout: 10 * time.Second}

// healthGet is a GET of path with header lines (`Name: value`); a transport error is a status 0 with its text.
func healthGet(d *daemon.Daemon, path string, headers ...string) Response {
	return healthGetBy(healthClient, d, path, headers...)
}

// healthGetBy is healthGet by a client of the caller's (healthPlainClient: one that asks for no compression).
func healthGetBy(client *http.Client, d *daemon.Daemon, path string, headers ...string) Response {
	req, err := http.NewRequest(http.MethodGet, d.BaseURL+path, nil)
	if err != nil {
		return Response{Body: []byte(err.Error())}
	}
	for _, hl := range headers {
		k, v, _ := strings.Cut(hl, ": ")
		req.Header.Set(k, v)
	}
	resp, err := client.Do(req)
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

// healthPlainClient sends no `Accept-Encoding`, as curl does by itself (the client of C's own test of the management
// API): the agent then answers without compression. It matters for an answer with an empty body: C sends such an
// answer without a length and closes, which this client reads as an empty body; to a client that accepts gzip (Go's
// own, a browser) C announces a chunked gzip body and sends no chunk, which the client reads as a broken transfer.
var healthPlainClient = &http.Client{Timeout: 10 * time.Second, Transport: &http.Transport{DisableCompression: true}}

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
	return h.keep(i, healthView(r, n.json(string(r.Body))), string(r.Body))
}

// keep remembers the answer a view of side i was rendered from, for near.
func (h *healthPair) keep(i int, view, raw string) string {
	h.raw[i][view] = raw
	return view
}

// keepClocks remembers the seconds a view of side i masked (healthVars, healthTrace), for near.
func (h *healthPair) keepClocks(i int, view string, clocks []int64) string {
	h.clocks[i][view] = clocks
	return view
}

// plain is side i's view of an answer compared as it is, but the side's directories.
func (h *healthPair) plain(i int, path string, headers ...string) string {
	r := healthGet(h.p.Each()[i].Daemon, path, headers...)
	return healthView(r, h.n[i].paths(string(r.Body)))
}

// dataAlerts is side i's view of the alert members of a v2 data answer (`/api/v2/data?<query>`), one per line: the two
// alert versions (`versions.alerts_hard_hash`: the host's alert dictionary's version; `alerts_soft_hash`: its
// transitions' count, database/contexts/query_target.c:1352-1353), `summary.alerts` (each alert name with its
// instances by status, formatters/jsonwrap-summary-alerts.c), and the alert counts of each node, context and instance
// of the summary (jsonwrap.c:118-143), named by the item's first member; an item without counts prints `none`.
// With `options=details` the answer has a tree of nodes, contexts and instances: each instance's `alerts` member (its
// chart's alerts at CLEAR or above, each with status, value and units, formatters/jsonwrap-objects-tree.c:6-30) is a
// line too, `none` for an instance without the member. The tree holds only instances with a queried dimension: a
// request without a window (ten minutes, aligned) queries none of a chart that is seconds old, so the checks that
// want the tree ask for the last seconds, unaligned.
// Everything else in the answer follows the data and the clock, and is left to the query checks.
func (h *healthPair) dataAlerts(i int, query string) string {
	r := healthGet(h.p.Each()[i].Daemon, "/api/v2/data?"+query)
	return healthView(r, healthDataAlerts(r.Body))
}

// healthDataAlerts renders the alert members of a v2 data body (dataAlerts); a body that is no JSON object is its text.
func healthDataAlerts(body []byte) string {
	doc, err := ParseJSON(body)
	if err != nil || doc.Kind != KindObject {
		return fmt.Sprintf("not a JSON object (%v): %s", err, body)
	}
	member := func(v Value, keys ...string) (Value, bool) {
		for _, m := range v.Members {
			if slices.Contains(keys, m.Key) {
				return m.Value, true
			}
		}
		return Value{}, false
	}
	text := func(v Value, ok bool) string {
		if !ok {
			return "none"
		}
		return v.String()
	}
	var out []string
	versions, _ := member(doc, "versions")
	for _, key := range []string{"alerts_hard_hash", "alerts_soft_hash"} {
		out = append(out, "versions."+key+": "+text(member(versions, key)))
	}
	summary, _ := member(doc, "summary")
	out = append(out, "summary.alerts: "+text(member(summary, "alerts")))
	for _, list := range []string{"nodes", "contexts", "instances"} {
		items, _ := member(summary, list)
		for k, item := range items.Items {
			name := "?"
			if len(item.Members) > 0 {
				name = item.Members[0].Key + "=" + item.Members[0].Value.String()
			}
			// the member's key is short, or long with `options=long-json-keys` (libnetdata/json/json-keys.h)
			out = append(out, fmt.Sprintf("summary.%s[%d] %s: %s", list, k, name, text(member(item, "al", "alerts"))))
		}
	}
	detailed, _ := member(doc, "detailed")
	nodes, _ := member(detailed, "nodes")
	for _, node := range nodes.Members {
		contexts, _ := member(node.Value, "contexts")
		for _, context := range contexts.Members {
			instances, _ := member(context.Value, "instances")
			for _, instance := range instances.Members {
				out = append(out, fmt.Sprintf("detailed %s %s: %s", context.Key, instance.Key, text(member(instance.Value, "alerts"))))
			}
		}
	}
	return strings.Join(out, "\n")
}

// healthMidSecond sleeps to the middle of a wall-clock second.
func healthMidSecond() { healthMidSecondOn(0) }

// healthMidSecondOn sleeps to the middle of the second before a whole second that is healthGridSecond past a
// multiple of grid (0: before any second), and returns that second.
func healthMidSecondOn(grid int64) int64 {
	now := time.Now()
	sec := now.Unix() + 1
	if !time.Unix(sec, 0).Add(-500 * time.Millisecond).After(now) {
		sec++
	}
	for grid > 0 && sec%grid != healthGridSecond%grid {
		sec++
	}
	time.Sleep(time.Until(time.Unix(sec, 0).Add(-500 * time.Millisecond)))
	return sec
}

// release ends the fake plugins' current phase once it has lasted `hold`: both get `file` at the middle of a second
// (with a grid: the second before the window's healthGridSecond), so both find it at the next whole second and both
// agents store the next phase's values from the same second on. It returns that second. The plugins' own records
// tell each side's: a difference, a second off the grid, or a plugin that did not reach the phase, is the harness's
// failure, not a verdict on the agents.
func (h *healthPair) release(t *testing.T, file string, phase int, hold time.Duration) int64 {
	t.Helper()
	if wait := time.Until(h.released.Add(hold)); wait > 0 {
		time.Sleep(wait)
	}
	want := healthMidSecondOn(h.c.grid)
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
			// the plugin's start of this run (a hand-back's second run has a second one)
			if len(starts) < max(h.run, 1) {
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
	if h.c.grid > 0 && secs[0] != want {
		t.Fatalf("harness: the sides switched to phase %d at second %d, not at %d (second %d of a window of %d)", phase, secs[0],
			want, healthGridSecond, h.c.grid)
	}
	return secs[0]
}

// create has both plugins create their chart at the same second, two seconds or more after both agents are ready,
// waits for the alert logs' first entries (settle), bounds the ids' seeds (bases) and returns the chart's first
// second. C runs no health on a host whose database holds nothing yet (database/rrdhost.c:964-969,
// rrdhost-status.c:124-131): with every collector off, HEALTH's first pass on localhost comes once this chart has
// data. It links the host's alerts (health_event_loop.c:258-284) and then runs the host's pending label recheck,
// which unlinks and links them again (:211-256): three entries per alert.
func (h *healthPair) create(t *testing.T) int64 {
	t.Helper()
	sec := h.createChart(t)
	h.settle(t, h.n, "")
	h.bases(t, h.n, sec)
	return sec
}

// createChart is create without the alert logs: for a case with health off, whose hosts have no alert log to wait for
// and no ids to bound. It returns the collected chart's first second.
func (h *healthPair) createChart(t *testing.T) int64 {
	t.Helper()
	time.Sleep(2 * time.Second)
	return h.release(t, "create", 0, 0)
}

// bases is the bound beside the id masks (D187 point 4): a host's unique ids and alarm ids count from the second its
// health started (sqlite_health.c:863-871), which the views print ids by. The oracle's are not before `first`, the
// second its chart's data began (one or two seconds after it in R75's 48 sides), and the candidate's, once it showed
// an id, within healthBound of the oracle's and not before `first` either: ids counted from anything else fail.
//
// The oracle's two bases must also be equal (D198 F1): a side that serves no alert log has its unique ids read by its
// alarm ids' base (healthNorm.noLog), which stands on C seeding both with one second (sqlite_health.c:864-871: two
// reads of the clock, one after the other). An oracle that shows two bases fails the case as `harness: …`: the run
// cannot judge such a candidate, and says nothing about either agent. A candidate without a log shows one base, the
// alarm ids', which is bound as any candidate's.
func (h *healthPair) bases(t *testing.T, n [2]*healthNorm, first int64) {
	t.Helper()
	if err := healthBases(n, first); err != nil {
		t.Fatal(err)
	}
}

// healthBases is bases' verdict: nil, or the failure's text.
func healthBases(n [2]*healthNorm, first int64) error {
	for _, b := range []struct {
		what   string
		set    [2]bool
		oracle int64
		cand   int64
	}{{"unique", [2]bool{n[0].uSet, n[1].uSet}, n[0].uBase, n[1].uBase}, {"alarm", [2]bool{n[0].aSet, n[1].aSet}, n[0].aBase, n[1].aBase}} {
		if !b.set[0] || b.oracle < first || b.oracle > first+10 {
			return fmt.Errorf("oracle: its %s ids count from %d (seen: %v): the chart's first second is %d", b.what, b.oracle, b.set[0], first)
		}
		if d := b.cand - b.oracle; b.set[1] && (d > healthBound || -d > healthBound || b.cand < first) {
			return fmt.Errorf("the %s ids' bases: oracle %d, candidate %d, the chart's first second %d: more than %d s apart, or before the chart",
				b.what, b.oracle, b.cand, first, healthBound)
		}
	}
	if n[0].uBase != n[0].aBase {
		return fmt.Errorf("harness: the oracle's unique ids count from %d, its alarm ids from %d: a side without an alert log is read "+
			"by one base for both, which this run's oracle does not have", n[0].uBase, n[0].aBase)
	}
	return nil
}

// the members of a hand-built v1 answer that hold the time of an event or a span between two events (not the time of
// the read: now, last_updated, next_update, db_after, db_before)
var healthEventRe = regexp.MustCompile(`"(when|delay_up_to_timestamp|exec_run|last_status_change|duration|non_clear_duration)":\s*"?(\d+)"?`)

// healthNear is the bound beside the clock masks (D187 point 4): two answers whose masked views are equal hold the
// same events, whose times, paired in the answers' order, are within healthBound of each other.
func healthNear(oracle, candidate string) error {
	mo, mc := healthEventRe.FindAllStringSubmatch(oracle, -1), healthEventRe.FindAllStringSubmatch(candidate, -1)
	if len(mo) != len(mc) {
		return fmt.Errorf("%d event times on the oracle, %d on the candidate", len(mo), len(mc))
	}
	for k := range mo {
		a, _ := strconv.ParseInt(mo[k][2], 10, 64)
		b, _ := strconv.ParseInt(mc[k][2], 10, 64)
		if mo[k][1] != mc[k][1] || b-a > healthBound || a-b > healthBound {
			return fmt.Errorf("event time %d: `%s` is %d on the oracle, `%s` is %d on the candidate: more than %d s apart", k+1,
				mo[k][1], a, mc[k][1], b, healthBound)
		}
	}
	return nil
}

// healthClocksNear is the bound beside the masks of the variables' endpoints: the seconds two equal views masked (the
// clock at the read, the last collection's second), paired in the answers' order, are within healthBound of each
// other. Both sides are read within a poll of each other and collect at the same second.
func healthClocksNear(oracle, candidate []int64) error {
	if len(oracle) != len(candidate) {
		return fmt.Errorf("%d masked seconds on the oracle, %d on the candidate", len(oracle), len(candidate))
	}
	for k := range oracle {
		if d := candidate[k] - oracle[k]; d > healthBound || -d > healthBound {
			return fmt.Errorf("masked second %d is %d on the oracle, %d on the candidate: more than %d s apart", k+1, oracle[k],
				candidate[k], healthBound)
		}
	}
	return nil
}

// near applies healthClocksNear to the seconds two equal views masked, and healthNear to the answers they were
// rendered from (a view no answer was kept for has none).
func (h *healthPair) near(oracle, candidate string) error {
	if err := healthClocksNear(h.clocks[0][oracle], h.clocks[1][candidate]); err != nil {
		return err
	}
	ro, ok := h.raw[0][oracle]
	rc, also := h.raw[1][candidate]
	if !ok || !also {
		return nil
	}
	return healthNear(ro, rc)
}

// settle waits until a host's alert log (localhost's for an empty prefix, else `/host/<name>`) shows its first
// entries on each side, so the side's id bases are the log's seeds: the entries logged when alerts are linked are
// stored by the metadata thread a few seconds later, after the first evaluations' (health_log.c:68-76). The log is
// settled when its entries run without a hole from its lowest id to the last id the host gave
// (`latest_alarm_log_unique_id`) and the lowest is an alert's first event. The oracle must get there (the case fails
// as `oracle: …`); the candidate gets the bounded wait and is then compared as it is. A candidate that does not serve
// the log has no entries to wait for: its ids' base is taken from its alerts (settleNoLog).
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
		if r := healthGet(h.p.Candidate, prefix+"/api/v1/alarm_log"); r.Status != http.StatusOK {
			h.settleNoLog(n, prefix)
			break
		}
	}
}

// settleNoLog is settle for a candidate that does not serve the alert log (D198 F1: C's log is SQLite's): the side's
// unique ids are read by its alarm ids' base (healthNorm.noLog), and that base comes from `/api/v1/alarms?all` once it
// shows an alarm id, within the bounded wait: the lowest id the candidate lists is taken for the lowest id the
// oracle lists (healthNorm.anchor), so the alerts listed are compared id for id and the base is bound to the oracle's
// (bases). A candidate that does not answer the alerts with 200 is compared at once, and so is one whose oracle lists
// no alert: its ids print `?`.
func (h *healthPair) settleNoLog(n [2]*healthNorm, prefix string) {
	n[1].noLog = true
	path := prefix + "/api/v1/alarms?all"
	listed, ok := healthLowestAlarm(string(healthGet(h.p.Oracle, path).Body))
	if !ok || !n[0].aSet {
		return
	}
	for end := time.Now().Add(healthCandidateWait); time.Now().Before(end); time.Sleep(250 * time.Millisecond) {
		r := healthGet(h.p.Candidate, path)
		if r.Status != http.StatusOK {
			return
		}
		if lowest, ok := healthLowestAlarm(string(r.Body)); ok {
			n[1].anchor(lowest, listed-n[0].aBase)
			return
		}
	}
}

// compareNow compares a view of both sides. The oracle's must satisfy `guard` within healthOracleWait (the case fails
// as `oracle: …` otherwise: the health state the comparison stands on was not reached); then the candidate has
// healthCandidateWait to show the same view, and the first difference left ends the case, naming what was compared
// with both views. While the candidate differs the oracle's view is taken again (a value still settling), and kept
// only under its guard. Two equal views must also hold their events' times within healthBound of each other (near).
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
			if err := h.near(oracle, candidate); err != nil {
				t.Fatalf("%s: the views are equal, the clocks behind them are not: %v\n%s", what, err, healthBrief(oracle))
			}
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

// threadRecords are side i's daemon.log records of one thread whose message starts with prefix, in file order, with
// the log masks (normalizeLog). The `errno` field is compared: no C record of these carried one in any run observed.
func (h *healthPair) threadRecords(t *testing.T, i int, thread, prefix string) []string {
	t.Helper()
	d := h.p.Each()[i].Daemon
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if threadOf(l) == thread && strings.Contains(l, ` msg="`+prefix) {
			out = append(out, normalizeLog(l, d.Opts.RunDir, ""))
		}
	}
	return out
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
	// the rule's hash, by which `/api/v2/alert_config` is asked
	Hash string `json:"config_hash_id"`
	// what a notification's arguments are read against (healthNorm.args, tid)
	Tid      string `json:"transition_id"`
	When     int64  `json:"when"`
	Duration int64  `json:"duration"`
	NonClear int64  `json:"non_clear_duration"`
	// the entry was made while its alert was silenced; the second of the repeat the entry is (0: a change of status)
	Silenced   bool  `json:"silenced"`
	LastRepeat int64 `json:"last_repeat"`
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

// entriesAs reads an alert log at path (a child's: `/host/<name>/api/v1/alarm_log`) with its host's normalizer.
func (h *healthPair) entriesAs(n *healthNorm, i int, path string) ([]healthEntry, error) {
	out, _, err := h.logAs(n, i, path)
	return out, err
}

// logAs is entriesAs with the answer's body (a view made of the entries keeps it for near).
func (h *healthPair) logAs(n *healthNorm, i int, path string) ([]healthEntry, string, error) {
	r := healthGet(h.p.Each()[i].Daemon, path)
	if r.Status != http.StatusOK {
		return nil, "", fmt.Errorf("%s answered %d %q", path, r.Status, r.Body)
	}
	n.observe(string(r.Body))
	var out []healthEntry
	if err := json.Unmarshal(r.Body, &out); err != nil {
		return nil, "", fmt.Errorf("%s: %v: %q", path, err, r.Body)
	}
	slices.SortFunc(out, func(a, b healthEntry) int { return int(a.UniqueID - b.UniqueID) })
	return out, string(r.Body), nil
}

// transitions is side i's alert log as per-alert transition lists: for each alert, in name order, its entries in
// log order as `name: OLD->NEW value`. A failed read is its text (the candidate's, compared against the oracle's
// lists).
func (h *healthPair) transitions(i int, query string) string {
	return h.transitionsAs(h.n[i], i, "/api/v1/alarm_log"+query)
}

// transitionsAs is transitions of the alert log at path, with its host's normalizer.
func (h *healthPair) transitionsAs(n *healthNorm, i int, path string) string {
	entries, raw, err := h.logAs(n, i, path)
	if err != nil {
		return err.Error()
	}
	return h.keep(i, strings.Join(healthTransitions(entries), "\n"), raw)
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
