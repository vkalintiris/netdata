// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/json"
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// pluginsOptions run the fake plugin: the oracle's stock plugins.d first (the primary directory: system-info.sh and
// the build info as in every other check; nothing installed there runs), the run directory's second, then `dirs`
// (`{run}` expands), a scan every second, `[plugin:difftest]` with an update every of `ue` and two options (dash splits
// them to `alpha` and `b c`), and the plugins `enable` names enabled too.
func pluginsOptions(ue int, dirs, enable []string, logs string) daemon.Options {
	stock := filepath.Join(filepath.Dir(os.Getenv("PARITY_ORACLE")), "..", "libexec", "netdata", "plugins.d")
	list := fmt.Sprintf("%q %q", filepath.Clean(stock), "{run}/plugins.d")
	for _, d := range dirs {
		list += fmt.Sprintf(" %q", d)
	}
	extra := "    " + plugin.Name + " = yes\n    check for new plugins every = 1\n"
	for _, name := range enable {
		extra += "    " + name + " = yes\n"
	}
	return daemon.Options{
		StreamMemoryMode: "ram",
		StorageTiers:     1,
		PulseOff:         true,
		PluginsDir:       list,
		PluginsExtra:     extra,
		ConfExtra:        fmt.Sprintf("[plugin:%s]\n    update every = %d\n    command options = alpha 'b c'\n", plugin.Name, ue),
		LogsExtra:        logs,
	}
}

var (
	pidRe     = regexp.MustCompile(`\bpid \d+`)
	requestRe = regexp.MustCompile(`\brequest (?:No )?\d+`)
	// thread ids inside messages: "my tid N, other collector tid N", "collector_tid N ... non-owner thread N",
	// "thread created with task id N"
	msgTidRe = regexp.MustCompile(`\b(tid|collector_tid|thread|task id) \d+`)
)

// pluginLogClasses are a daemon's records about the fake plugins, per class, each in file order (several processes
// write the collectors log): each fake plugin thread's (`PD[difftest…`) in daemon.log, the plugins thread's, each fake
// plugin thread's in collector.log, the spawn server's naming a fake plugin, and the raw lines they wrote to stderr.
// Pids, spawn request numbers and thread ids in messages are masked (C counts every spawn, system-info.sh's included).
func pluginLogClasses(t *testing.T, d *daemon.Daemon) map[string][]string {
	t.Helper()
	return pluginLogClassesBefore(t, d, time.Time{})
}

// pluginLogClassesBefore is pluginLogClasses with only the records written before `stop` (all when zero; a line
// without a time, such as a plugin's raw stderr, is kept): what a daemon logged before its stop began.
func pluginLogClassesBefore(t *testing.T, d *daemon.Daemon, stop time.Time) map[string][]string {
	t.Helper()
	return pluginLogClassesOf(t, d, stop, fakePluginThread, plugin.Name)
}

// fakePluginThread holds for a fake plugin's thread (`PD[difftest…`).
func fakePluginThread(th string) bool { return strings.HasPrefix(th, "PD["+plugin.Name) }

// pluginLogClassesOf is pluginLogClassesBefore of the plugin threads `thread` holds for, with the spawn server's
// records naming `spawned` (the fake plugin's name, or the real plugins' directory).
func pluginLogClassesOf(t *testing.T, d *daemon.Daemon, stop time.Time, thread func(string) bool,
	spawned string) map[string][]string {
	t.Helper()
	classes := map[string][]string{}
	add := func(class, line string) {
		line = normalizeLog(line, d.Opts.RunDir, "")
		line = pidRe.ReplaceAllString(line, "pid P")
		line = msgTidRe.ReplaceAllString(line, "$1 N")
		classes[class] = append(classes[class], requestRe.ReplaceAllString(line, "request R"))
	}
	pluginThread := func(l string) string {
		if th := threadOf(l); thread(th) {
			return "thread=" + th
		}
		return ""
	}
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if !recordBefore(l, stop) {
			continue
		}
		switch {
		case pluginThread(l) != "":
			add("daemon.log "+pluginThread(l), l)
		case strings.Contains(l, "thread=PLUGINSD"):
			add("daemon.log thread=PLUGINSD", l)
		}
	}
	for _, l := range logLines(t, d.Opts.RunDir, "collector.log") {
		if !recordBefore(l, stop) {
			continue
		}
		switch {
		case pluginThread(l) != "":
			add("collector.log "+pluginThread(l), l)
		case strings.Contains(l, "comm=spawn-plugins") && strings.Contains(l, spawned):
			// whether the PD thread closes the status socket before the server reaps the killed plugin is a race
			// (spawn_server_nofork.c:254-257, :1599, :1744)
			if !strings.Contains(l, "Cannot send exit status") {
				add("collector.log comm=spawn-plugins", l)
			}
		case !strings.HasPrefix(l, "time="):
			add("collector.log raw", l)
		}
	}
	return classes
}

// otherPluginThreads are the plugin threads other than the fake plugins' in a daemon's logs: none may run.
func otherPluginThreads(t *testing.T, d *daemon.Daemon) []string {
	var out []string
	for _, name := range []string{"daemon.log", "collector.log"} {
		for _, l := range logLines(t, d.Opts.RunDir, name) {
			if th := threadOf(l); strings.HasPrefix(th, "PD[") && !strings.HasPrefix(th, "PD["+plugin.Name) && !slices.Contains(out, th) {
				out = append(out, th)
			}
		}
	}
	return out
}

// pluginPoints are a chart's non-null points over [first, last], times rebased to first.
func pluginPoints(t *testing.T, d *daemon.Daemon, chart string, first, last int64) []string {
	t.Helper()
	path := fmt.Sprintf("/api/v1/data?chart=%s&after=%d&before=%d&format=json&options=unaligned", chart, first-1, last)
	b, err := rawExchange(d.Addr, []byte("GET "+path+" HTTP/1.1\r\n\r\n"), 5*time.Second)
	if err != nil {
		t.Fatalf("%s: %v", path, err)
	}
	var data struct{ Data [][]any }
	if err := json.Unmarshal(httpBody(b), &data); err != nil {
		t.Fatalf("%s: %v: %s", path, err, b)
	}
	var out []string
	for _, row := range data.Data {
		if len(row) == 2 && row[1] != nil {
			out = append(out, fmt.Sprintf("%v@%d", row[1], int64(row[0].(float64))-first))
		}
	}
	slices.Sort(out)
	return out
}

// pluginCase is one scenario and what is checked while it plays, before both daemons stop; the starts, their views
// and the log classes are compared after.
type pluginCase struct {
	sc plugin.Scenario
	// ue is the plugin's `update every` (1 when 0)
	ue int
	// more are other fake plugins installed before the start; dirs are more plugin directories (`{run}` expands);
	// enable are more names enabled in [plugins]
	more   []morePlugin
	dirs   []string
	enable []string
	// logs is a [logs] section (e.g. the debug level)
	logs string
	// play waits for each side's plugin and compares what is visible while it runs, with the oracle's guards
	play func(t *testing.T, p *Pair, ls [2]plugin.Layout)
	// guard checks the oracle's records and log classes after the stop
	guard func(t *testing.T, starts [][]plugin.Record, classes map[string][]string)
	// mask, when set, rewrites each side's log classes after the guard, before they are compared: C's run-to-run
	// variation only; views, when set, rewrites each side's difftest start views (maskedViews') the same way
	mask  func(classes map[string][]string)
	views func(vs []plugin.View)
	// adjust, when set, changes both sides' options (e.g. a [web] setting); prepare, when set, lays out more files in
	// each run directory before its daemon starts (e.g. bearer token files)
	adjust  func(o *daemon.Options)
	prepare func(t *testing.T, runDir string)
	// after, when set, runs once both daemons stopped and the starts and log classes were compared: a check's own
	// comparisons of what the daemons logged
	after func(t *testing.T, p *Pair)
}

// morePlugin is another fake plugin: its file in a directory under the run directory, and its scenario.
type morePlugin struct {
	dir, file string
	sc        plugin.Scenario
}

// installMore installs a fake plugin on each side (a late one, during play) and returns its layouts.
func installMore(t *testing.T, p *Pair, m morePlugin) [2]plugin.Layout {
	t.Helper()
	engine, err := plugin.Engine()
	if err != nil {
		t.Fatal(err)
	}
	var ls [2]plugin.Layout
	for i, side := range p.Each() {
		run := side.Daemon.Opts.RunDir
		if ls[i], err = plugin.InstallAs(run, filepath.Join(run, m.dir), m.file, engine, m.sc); err != nil {
			t.Fatal(err)
		}
	}
	return ls
}

// moreLayouts are each side's layout of another fake plugin (InstallAs's).
func moreLayouts(p *Pair, dir, file string) [2]plugin.Layout {
	var ls [2]plugin.Layout
	for i, side := range p.Each() {
		run := side.Daemon.Opts.RunDir
		ls[i] = plugin.Layout{PluginsDir: filepath.Join(run, dir), Dir: plugin.MoreDir(run, filepath.Join(run, dir), file)}
	}
	return ls
}

// pdThreads are the names (`/proc/<pid>/task/*/comm`) of a daemon's plugin threads, sorted.
func pdThreads(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	comms, err := filepath.Glob(fmt.Sprintf("/proc/%d/task/*/comm", d.PID()))
	if err != nil {
		t.Fatal(err)
	}
	var out []string
	for _, c := range comms {
		b, err := os.ReadFile(c)
		if err == nil && strings.HasPrefix(string(b), "PD[") {
			out = append(out, strings.TrimSpace(string(b)))
		}
	}
	slices.Sort(out)
	return out
}

// shortRunRe is a daemon's runtime directory, made by os.MkdirTemp under $TMPDIR (TMPDIR moves it off /tmp).
var shortRunRe = regexp.MustCompile(regexp.QuoteMeta(filepath.Join(os.TempDir(), "ndrun-")) + `\d+`)

// sigQueuedRe is the count of the user's queued signals in the wrapper's SigQ line: any process of the user moves it.
var sigQueuedRe = regexp.MustCompile(`^SigQ:\s+\d+/`)

// startContext is what a start recorded of the context it began in, with each side's directories masked: its
// environment, descriptors (kinds), the wrapper's snapshot (signals, capabilities, limits), directory, parent, stdio
// flags, OOM score, nice, scheduling policy, and whether its process group and session are the daemon's.
func startContext(r plugin.Record, d *daemon.Daemon) map[string][]string {
	mask := func(v string) string {
		v = sigQueuedRe.ReplaceAllString(v, "SigQ: N/")
		return shortRunRe.ReplaceAllString(strings.ReplaceAll(v, d.Opts.RunDir, "<RUN>"), "<RT>")
	}
	var env, fds, pre []string
	for _, e := range r.Env {
		env = append(env, mask(e))
	}
	for _, f := range r.Fds {
		num, target, _ := strings.Cut(f, " ")
		fds = append(fds, num+" "+mask(fdKind(target)))
	}
	for _, l := range strings.Split(strings.TrimRight(r.Pre, "\n"), "\n") {
		pre = append(pre, mask(l))
	}
	return map[string][]string{
		"env":    maskEnv(env),
		"fds":    fds,
		"pre":    pre,
		"cwd":    {mask(r.Cwd)},
		"parent": {r.ParentComm},
		"stdio":  r.StdioFlags,
		"oom":    {r.OomScoreAdj},
		"nice":   {r.Nice},
		"sched":  {r.SchedPolicy},
		"ids":    {fmt.Sprint(fmt.Sprintf("%d %d", r.Pgid, r.Sid) == daemonIDs(d.PID()))},
	}
}

// waitPlugin waits up to 30 s on each side until ok holds of its starts, and returns them.
func waitPlugin(t *testing.T, p *Pair, ls [2]plugin.Layout, what string, ok func([][]plugin.Record) bool) [2][][]plugin.Record {
	t.Helper()
	var out [2][][]plugin.Record
	for i, side := range p.Each() {
		starts, done := ls[i].WaitFor(30*time.Second, ok)
		if !done {
			t.Fatalf("%s: %s within 30 s: %d starts", side.Role, what, len(starts))
		}
		out[i] = starts
	}
	return out
}

// startEnded holds once start n (from 1) wrote its end.
func startEnded(n int) func([][]plugin.Record) bool {
	return func(s [][]plugin.Record) bool { return len(s) >= n && plugin.Has(s[n-1], "end", "") }
}

// startHangs holds once start n (from 1) reached its hang step.
func startHangs(n int) func([][]plugin.Record) bool {
	return func(s [][]plugin.Record) bool { return len(s) >= n && plugin.Has(s[n-1], "step", "hang") }
}

// checkRestart compares each side's time from start n-1's end to start n's start (n from 2) with C's band [lo, hi].
func checkRestart(t *testing.T, starts [2][][]plugin.Record, n int, lo, hi time.Duration, why string) {
	t.Helper()
	var gaps [2]time.Duration
	for i := range starts {
		gaps[i] = plugin.Time(starts[i][n-1], "start").Sub(plugin.Time(starts[i][n-2], "end"))
	}
	t.Logf("start %d after the previous end: oracle %v, candidate %v", n, gaps[0], gaps[1])
	for i, g := range gaps {
		if g < lo || g > hi {
			t.Errorf("%s: start %d came %v after the previous end; C's band is [%v, %v] (%s)", []Role{Oracle, Candidate}[i], n, g, lo, hi, why)
		}
	}
}

// hasRecord reports whether a log class of the oracle has a record containing text.
func hasRecord(t *testing.T, classes map[string][]string, class, text string) {
	t.Helper()
	if !slices.ContainsFunc(classes[class], func(l string) bool { return strings.Contains(l, text) }) {
		t.Errorf("oracle: %s has no %q: %q", class, text, classes[class])
	}
}

// maskStopWalk hides what races when a stop finds several plugin threads alive. PLUGINSD's cleanup walks the plugins
// newest first, naming each one still enabled and running, then cancelling and joining it (plugins_d.c:204-228,
// :434); a thread blocked on its plugin sees the cancel at its next 100 ms poll (socket.c:448-462), and one sleeping
// between starts sees the exit itself (plugins_d.c:9-17, daemon-service.c:111). Meanwhile the shutdown's stop of the
// collectors (daemon-shutdown.c:227) cancels every thread the walk has not reached, at once (daemon-service.c:186-187);
// each kills its plugin and disables itself (plugins_d.c:87-91, :190). Whether the walk still names a thread, and the
// order of the plugins' kills, are races: the walk's records are dropped, the spawn server's compared as a set.
func maskStopWalk(classes map[string][]string) {
	slices.Sort(classes["collector.log comm=spawn-plugins"])
	if lines, ok := classes["daemon.log thread=PLUGINSD"]; ok {
		classes["daemon.log thread=PLUGINSD"] = slices.DeleteFunc(lines, func(l string) bool {
			return strings.Contains(l, "stopping plugin thread")
		})
	}
}

// maskStopWalkAfterFirst is maskStopWalk keeping the walk's first record: the newest plugin, which the walk reaches
// before the collectors' cancel can (it starts within 100 ms of the stop, the cancel later; I10's 60 stops).
func maskStopWalkAfterFirst(classes map[string][]string) {
	slices.Sort(classes["collector.log comm=spawn-plugins"])
	if lines, ok := classes["daemon.log thread=PLUGINSD"]; ok {
		first := true
		classes["daemon.log thread=PLUGINSD"] = slices.DeleteFunc(lines, func(l string) bool {
			if !strings.Contains(l, "stopping plugin thread") {
				return false
			}
			keep := first
			first = false
			return !keep
		})
	}
}

// fnRegisterChanged is fnRegister with another help text (`reregister`).
const fnRegisterChanged = `FUNCTION GLOBAL "difftest-fn" 10 "parity fn changed" "top" "0x13" 100 1` + "\n"

// TestPluginsFakePlugin (check `plugins.fake-plugin`, M8 commit 0, D134): both agents run `difftest.plugin`
// (package plugin): a dash wrapper exec'ing a Go engine that plays a scenario and records its argv, its stdin and how
// it ended. Per case: what the agents show while it runs, then (both stopped) each start's view, the number of starts
// and the plugin's log records in five classes. Every guard runs on the oracle, so no case passes with two sides that
// never ran the plugin; neither side may start an installed plugin.
func TestPluginsFakePlugin(t *testing.T) {
	cases := map[string]pluginCase{
		// five collections, held until released, then exit 0: the chart while held, gone after; a restart after one
		// update every; the second start killed by the agent's stop (QUIT, then SIGTERM and SIGKILL at once from the
		// cancelled plugin thread: what the plugin records of them is a race, so views end at the stop)
		"collect": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{{Collect: &plugin.Collect{Chart: "difftest.a", Dims: []string{"x"}, N: 5}},
					{WaitFile: "release-1"}, {Exit: plugin.ExitCode(0)}}},
				{Steps: []plugin.Step{{Hang: true}}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				var charts, points [2][]string
				for i, side := range p.Each() {
					starts, ok := ls[i].WaitFor(30*time.Second, func(s [][]plugin.Record) bool {
						return len(s) >= 1 && plugin.Has(s[0], "waiting", "release-1")
					})
					if !ok {
						t.Fatalf("%s: the plugin did not collect and wait within 30 s: %d starts", side.Role, len(starts))
					}
					var secs []int64
					for _, r := range starts[0] {
						if r.Kind == "collected" {
							secs = append(secs, r.Sec)
						}
					}
					time.Sleep(1500 * time.Millisecond)
					ids, _, err := hostCharts(side.Daemon, "")
					if err != nil {
						t.Fatal(err)
					}
					slices.Sort(ids)
					charts[i] = ids
					points[i] = pluginPoints(t, side.Daemon, "difftest.a", secs[0], secs[len(secs)-1])
				}
				if !slices.Contains(charts[0], "difftest.a") || len(points[0]) < 3 {
					t.Errorf("oracle while held: charts %v, points %v", charts[0], points[0])
				}
				diffLines(t, "charts while held", charts[0], charts[1])
				diffLines(t, "points while held", points[0], points[1])
				// C's label order follows its dictionary's: varies between runs
				rules := Rules{Unordered: []string{"chart_labels"}, Masks: []Mask{
					{Pattern: "last_updated", Reason: "each side's own collection time"},
					{Pattern: "first_entry", Reason: "each side's own collection time"},
					{Pattern: "last_entry", Reason: "each side's own collection time"},
				}}
				diffs, err := p.CompareJSON("/api/v1/chart?chart=difftest.a", nil, rules)
				if err != nil {
					t.Fatal(err)
				}
				for _, d := range diffs {
					t.Errorf("/api/v1/chart?chart=difftest.a: %s", d)
				}
				var gaps [2]time.Duration
				for i, side := range p.Each() {
					if err := ls[i].Release("release-1"); err != nil {
						t.Fatal(err)
					}
					starts, ok := ls[i].WaitFor(30*time.Second, func(s [][]plugin.Record) bool {
						return len(s) >= 2 && plugin.Has(s[1], "step", "hang")
					})
					if !ok {
						t.Fatalf("%s: no second start within 30 s of the exit: %d starts", side.Role, len(starts))
					}
					gaps[i] = plugin.Time(starts[1], "start").Sub(plugin.Time(starts[0], "end"))
					ids, _, err := hostCharts(side.Daemon, "")
					if err != nil {
						t.Fatal(err)
					}
					slices.Sort(ids)
					charts[i] = ids
				}
				t.Logf("restarts after the exit: oracle %v, candidate %v", gaps[0], gaps[1])
				for i, g := range gaps {
					if g < time.Second || g > 2500*time.Millisecond {
						t.Errorf("%s: restarted %v after the exit, C's band is one update every to 2.5 s", []Role{Oracle, Candidate}[i], g)
					}
				}
				if slices.Contains(charts[0], "difftest.a") {
					t.Errorf("oracle: the chart is still shown after its plugin exited: %v", charts[0])
				}
				diffLines(t, "charts after the exit", charts[0], charts[1])
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				// the agent's stop kills the hanging second start (SIGKILL: the plugin's SIGTERM handler delays its
				// death past the second kill)
				if len(starts) != 2 || !slices.ContainsFunc(classes["collector.log comm=spawn-plugins"], func(l string) bool {
					return strings.Contains(l, "killed by signal 9")
				}) {
					t.Errorf("oracle: %d starts, the spawn server's records %q", len(starts), classes["collector.log comm=spawn-plugins"])
				}
				if !slices.ContainsFunc(classes["collector.log thread=PD[difftest]"], func(l string) bool {
					return strings.Contains(l, "disconnected after 5 successful data collections.")
				}) {
					t.Errorf("oracle: no \"disconnected after 5 successful data collections.\" record")
				}
			},
		},
		// a line on stderr and exit 3 with nothing collected: disabled, never started again
		"fail": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{{Stderr: "difftest: failing early"}, {Exit: plugin.ExitCode(3)}}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				for i, side := range p.Each() {
					if _, ok := ls[i].WaitFor(30*time.Second, func(s [][]plugin.Record) bool {
						return len(s) >= 1 && plugin.Has(s[0], "end", "")
					}); !ok {
						t.Fatalf("%s: the plugin did not start and exit within 30 s", side.Role)
					}
				}
				// at least five scans at one second: a disabled plugin is never started again
				time.Sleep(6 * time.Second)
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 1 {
					t.Errorf("oracle: %d starts", len(starts))
				}
				for class, want := range map[string]string{
					"daemon.log thread=PD[difftest]":   "exited with error code 3 and haven't collected any data. Disabling it.",
					"collector.log comm=spawn-plugins": "exited with exit code 3",
					"collector.log raw":                "difftest: failing early",
				} {
					if !slices.ContainsFunc(classes[class], func(l string) bool { return strings.Contains(l, want) }) {
						t.Errorf("oracle: %s has no %q: %q", class, want, classes[class])
					}
				}
			},
		},
		// one collection, then a keyword nobody knows (line 6): the parser fails, the plugin is stopped with QUIT and
		// started again
		"badline": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{{Collect: &plugin.Collect{Chart: "difftest.b", Dims: []string{"x"}, N: 1}},
					{Emit: "DIFFTEST_UNKNOWN a 'b c'\n"}, {Hang: true}}},
				{Steps: []plugin.Step{{Hang: true}}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				var gaps [2]time.Duration
				for i, side := range p.Each() {
					starts, ok := ls[i].WaitFor(30*time.Second, func(s [][]plugin.Record) bool {
						return len(s) >= 2 && plugin.Has(s[1], "step", "hang")
					})
					if !ok {
						t.Fatalf("%s: no second start within 30 s: %d starts", side.Role, len(starts))
					}
					gaps[i] = plugin.Time(starts[1], "start").Sub(plugin.Time(starts[0], "end"))
				}
				t.Logf("restarts after the bad line's stop: oracle %v, candidate %v", gaps[0], gaps[1])
				// C kills the plugin, then sleeps one update every (plugins_d.c:172-175)
				for i, g := range gaps {
					if g < time.Second || g > 2500*time.Millisecond {
						t.Errorf("%s: restarted %v after the stop, C's band is one update every to 2.5 s", []Role{Oracle, Candidate}[i], g)
					}
				}
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 2 || !strings.Contains(plugin.ViewOf(starts[0]).Stdin, "QUIT") {
					t.Errorf("oracle: %d starts, the first's stdin %q", len(starts), plugin.ViewOf(starts[0]).Stdin)
				}
				if !slices.ContainsFunc(classes["daemon.log thread=PD[difftest]"], func(l string) bool {
					return strings.Contains(l, "parser_action('DIFFTEST_UNKNOWN') failed on line 6")
				}) {
					t.Errorf("oracle: no parser failure on line 6: %q", classes["daemon.log thread=PD[difftest]"])
				}
			},
		},
		// exit 0 without data: C waits ten update everies before the next start (plugins_d.c:66-73); the stop comes
		// during the second wait, which it ends without a kill
		"exit0-nodata": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Exit: plugin.ExitCode(0)}}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not start and exit", startEnded(1))
				starts := waitPlugin(t, p, ls, "no second start", startEnded(2))
				checkRestart(t, starts, 2, 10*time.Second, 11500*time.Millisecond, "ten update everies after an exit 0 without data")
				time.Sleep(time.Second)
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 2 {
					t.Errorf("oracle: %d starts", len(starts))
				}
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", "does not generate useful output but it reports success (exits with 0). Waiting a bit before starting it again.")
			},
			// the stop finds the plugin's thread in its ten-update-every sleep (plugins_d.c:72); it and PLUGINSD's scan
			// sleep poll the exit every 100 ms on their own schedules (:9-17), so whether the cleanup still sees it
			// running and names it (:132, :190, :216-217) is a race, C against C too
			mask: maskStopWalk,
		},
		// one collection, then exit 3: useful output in the past, so a start again after ten update everies
		"data-then-error": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{{Collect: &plugin.Collect{Chart: "difftest.e", Dims: []string{"x"}, N: 1}}, {Exit: plugin.ExitCode(3)}}},
				{Steps: []plugin.Step{{Hang: true}}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not collect and exit", startEnded(1))
				starts := waitPlugin(t, p, ls, "no second start", startHangs(2))
				checkRestart(t, starts, 2, 10*time.Second, 11500*time.Millisecond, "ten update everies after an error with data in the past")
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 2 {
					t.Errorf("oracle: %d starts", len(starts))
				}
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", "exited with error code 3, but has given useful output in the past (1 times). Waiting a bit before starting it again.")
			},
		},
		// a FUNCTION alone counts as a data collection: a start again after one update every
		"fn-only": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{{Emit: "FUNCTION GLOBAL 'difftest-fn' 10 'a test function' 'top' 'member' 100 3\n"}, {SleepMs: 300}, {Exit: plugin.ExitCode(0)}}},
				{Steps: []plugin.Step{{Hang: true}}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				starts := waitPlugin(t, p, ls, "no second start", startHangs(2))
				checkRestart(t, starts, 2, time.Second, 2500*time.Millisecond, "one update every after a run with a collection")
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				hasRecord(t, classes, "collector.log thread=PD[difftest]", "disconnected after 1 successful data collections.")
			},
		},
		// DISABLE: stopped with QUIT, never started again; no data, so C says so and sleeps, until the agent's stop
		"disable": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Emit: "DISABLE\n"}, {Hang: true}}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not end", startEnded(1))
				time.Sleep(2 * time.Second)
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 1 || plugin.ViewOf(starts[0]).Stdin != "QUIT" {
					t.Errorf("oracle: %d starts, %+v", len(starts), starts)
				}
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", "PLUGINSD: plugin called DISABLE. Disabling it.")
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", "Will not start it again - it is now disabled.")
			},
		},
		// EXIT after a collection: stopped with QUIT, started again after one update every
		"exit": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{{Collect: &plugin.Collect{Chart: "difftest.x", Dims: []string{"x"}, N: 1}}, {Emit: "EXIT\n"}, {Hang: true}}},
				{Steps: []plugin.Step{{Hang: true}}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				starts := waitPlugin(t, p, ls, "no second start", startHangs(2))
				checkRestart(t, starts, 2, time.Second, 2500*time.Millisecond, "one update every after a run with a collection")
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", "PLUGINSD: plugin called EXIT.")
			},
		},
		// TRUST_DURATIONS with a value other than 0 or 1 disables the plugin with C's reason
		"trust-durations": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Emit: "TRUST_DURATIONS 2\n"}, {Hang: true}}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not end", startEnded(1))
				time.Sleep(2 * time.Second)
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 1 {
					t.Errorf("oracle: %d starts", len(starts))
				}
				hasRecord(t, classes, "collector.log thread=PD[difftest]", "PLUGINSD: keyword TRUST_DURATIONS: parameter must be 0 or 1")
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", "parser_action('TRUST_DURATIONS') failed on line 1")
			},
		},
		// a streaming-only keyword is unknown to a plugin's parser: an error, so a start again (not disabled)
		"begin2": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{{Collect: &plugin.Collect{Chart: "difftest.v", Dims: []string{"x"}, N: 1}}, {Emit: "BEGIN2 'difftest.v' 1 10 #\n"}, {Hang: true}}},
				{Steps: []plugin.Step{{Hang: true}}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				starts := waitPlugin(t, p, ls, "no second start", startHangs(2))
				checkRestart(t, starts, 2, time.Second, 2500*time.Millisecond, "one update every after a run with a collection")
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", "parser_action('BEGIN2') failed on line 6")
			},
		},
		// FLUSH ends the chart's scope: the SET after it has no chart, which disables the plugin
		"flush": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{{Collect: &plugin.Collect{Chart: "difftest.f", Dims: []string{"x"}, N: 1}}, {Emit: "BEGIN difftest.f\nFLUSH\nSET x = 5\n"}, {Hang: true}}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not end", startEnded(1))
				time.Sleep(3 * time.Second)
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 1 {
					t.Errorf("oracle: %d starts", len(starts))
				}
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", "PLUGINSD: command SET requires a chart defined via command CHART, but is not set.")
			},
		},
		// HOST localhost and a CONFIG with an action nobody knows: a warning, counted as a collection
		"config": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{{Emit: "HOST localhost\nCONFIG difftest:x bogus\n"}, {SleepMs: 300}, {Exit: plugin.ExitCode(0)}}},
				{Steps: []plugin.Step{{Hang: true}}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				starts := waitPlugin(t, p, ls, "no second start", startHangs(2))
				checkRestart(t, starts, 2, time.Second, 2500*time.Millisecond, "one update every after a run with a collection")
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				hasRecord(t, classes, "collector.log thread=PD[difftest]", "DYNCFG: unknown action 'bogus' received from plugin")
			},
		},
		// `update every = 2`: the plugin's argument, and two seconds before a start again
		"ue2": {
			ue: 2,
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{{Collect: &plugin.Collect{Chart: "difftest.u", Dims: []string{"x"}, N: 1}}, {Exit: plugin.ExitCode(0)}}},
				{Steps: []plugin.Step{{Hang: true}}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				starts := waitPlugin(t, p, ls, "no second start", startHangs(2))
				checkRestart(t, starts, 2, 2*time.Second, 3500*time.Millisecond, "one update every of 2 s after a run with a collection")
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {},
		},
		// at the debug level: "connected to", QUIT's record and the kill's records, start and stop
		"debug": {
			logs: "    level = debug\n",
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{{Collect: &plugin.Collect{Chart: "difftest.g", Dims: []string{"x"}, N: 1}}, {Emit: "DIFFTEST_UNKNOWN\n"}, {Hang: true}}},
				{Steps: []plugin.Step{{Hang: true}}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				starts := waitPlugin(t, p, ls, "no second start", startHangs(2))
				checkRestart(t, starts, 2, time.Second, 2500*time.Millisecond, "one update every after a run with a collection")
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", "PLUGINSD: 'host:parity-parent' connected to '<RUN>/plugins.d/difftest.plugin' running on pid P")
				hasRecord(t, classes, "collector.log thread=PD[difftest]", "PLUGINSD: sending 'QUIT'  to plugin: difftest.plugin")
			},
		},
		// M8 commit 11 (D182.4): a method registered again, at the debug level. The same line again logs nothing; a
		// changed help logs nRPC's "re-registered with changes" (nrpc_registry_conflict_cb, nrpc-registry.c:137-176),
		// once, and its repeat nothing again. difftest-open's registration comes last: listed, every line before it was
		// read
		"reregister": {
			logs: "    level = debug\n",
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				{Emit: fnRegister + fnRegister + fnRegisterChanged + fnRegisterChanged + fnOpenRegister}, {Hang: true}}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not hang", startHangs(1))
				for i, side := range p.Each() {
					x := &fnHTTPSide{role: side.Role, d: side.Daemon, l: ls[i]}
					x.listed(t, []fnListed{{"", "difftest-open"}}, "")
				}
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				const class = "daemon.log thread=PD[difftest]"
				hasRecord(t, classes, class, "NRPC: method 'difftest-fn' of host 0xPTR re-registered with changes")
				if n := len(slices.DeleteFunc(slices.Clone(classes[class]), func(l string) bool {
					return !strings.Contains(l, "re-registered")
				})); n != 1 {
					t.Errorf("oracle: %d re-registration records, want 1: %q", n, classes[class])
				}
			},
		},
		// the context a plugin starts in (startContext), against C's
		"context": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Hang: true}}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				starts := waitPlugin(t, p, ls, "the plugin did not hang", startHangs(1))
				var ctx [2]map[string][]string
				for i, side := range p.Each() {
					ctx[i] = startContext(starts[i][0][0], side.Daemon)
				}
				if got := ctx[0]["parent"]; !slices.Equal(got, []string{"spawn-plugins"}) || ctx[0]["ids"][0] != "true" {
					t.Errorf("oracle: parent %q, the daemon's process group and session %q", got, ctx[0]["ids"])
				}
				for _, k := range slices.Sorted(maps.Keys(ctx[0])) {
					diffLines(t, "the start's "+k, ctx[0][k], ctx[1][k])
				}
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {},
		},
		// a plugin installed while the agents run starts at the next scan; the stop's cleanup names both, newest first
		"late": {
			sc:     plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Hang: true}}}}},
			enable: []string{"difftestlate"},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not hang", startHangs(1))
				installed := time.Now()
				late := installMore(t, p, morePlugin{dir: "plugins.d", file: "difftestlate.plugin", sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Hang: true}}}}}})
				starts := waitPlugin(t, p, late, "the late plugin did not start", startHangs(1))
				for i, s := range starts {
					if d := plugin.Time(s[0], "start").Sub(installed); d > 3*time.Second {
						t.Errorf("%s: the late plugin started %v after its install; the scan runs every second", []Role{Oracle, Candidate}[i], d)
					}
				}
			},
			// the walk starts with the newest plugin, which it always names; whether it still names the older one
			// races the collectors' cancel, in C too (D163)
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				stopping := slices.DeleteFunc(slices.Clone(classes["daemon.log thread=PLUGINSD"]), func(l string) bool {
					return !strings.Contains(l, "stopping plugin thread")
				})
				if len(stopping) == 0 || len(stopping) > 2 || !strings.Contains(stopping[0], "plugin:difftestlate") {
					t.Errorf("oracle: the stopping records, newest first: %q", stopping)
				}
			},
			mask: maskStopWalkAfterFirst,
		},
		// which files are plugins: the three suffixes, a plugin in a later directory, the same file in two (the first
		// wins), a name too long for a thread's tag, files that are not plugins, an obsolete plugin and a missing
		// directory (reported once)
		"names": {
			sc:     plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Hang: true}}}}},
			dirs:   []string{"{run}/plugins2.d", "{run}/nope.d"},
			enable: []string{"difftestu", "difftestd", "difftest-longname", "difftestx"},
			more: []morePlugin{
				{dir: "plugins.d", file: "difftestu_plugin", sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Hang: true}}}}}},
				{dir: "plugins.d", file: "difftestd-plugin", sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Hang: true}}}}}},
				{dir: "plugins.d", file: "difftest-longname.plugin", sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Hang: true}}}}}},
				{dir: "plugins.d", file: ".plugin", sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Hang: true}}}}}},
				{dir: "plugins.d", file: "difftestoff.plugin.off", sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Hang: true}}}}}},
				{dir: "plugins.d", file: "otel-signal-viewer.plugin", sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Hang: true}}}}}},
				{dir: "plugins2.d", file: "difftestu_plugin", sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Hang: true}}}}}},
				{dir: "plugins2.d", file: "difftestx.plugin", sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Hang: true}}}}}},
			},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not hang", startHangs(1))
				for _, m := range [][2]string{{"plugins.d", "difftestu_plugin"}, {"plugins.d", "difftestd-plugin"},
					{"plugins.d", "difftest-longname.plugin"}, {"plugins2.d", "difftestx.plugin"}} {
					waitPlugin(t, p, moreLayouts(p, m[0], m[1]), m[1]+" did not hang", startHangs(1))
				}
				// two more scans: nothing else starts
				time.Sleep(2500 * time.Millisecond)
				var threads [2][]string
				for i, side := range p.Each() {
					threads[i] = pdThreads(t, side.Daemon)
				}
				if !slices.Contains(threads[0], "PD[difftest-lon") {
					t.Errorf("oracle: the plugin threads %q", threads[0])
				}
				diffLines(t, "plugin threads", threads[0], threads[1])
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				hasRecord(t, classes, "daemon.log thread=PLUGINSD", "cannot open plugins directory '<RUN>/nope.d'")
				hasRecord(t, classes, "daemon.log thread=PLUGINSD", "skipping obsolete plugin 'otel-signal-viewer.plugin'")
			},
			// five plugins hang at the stop
			mask: maskStopWalk,
		},
		// two plugins define the same chart: the first, collecting it again after the second took it over, is
		// collected twice and disabled
		"twice": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				{Emit: "CHART difftest.tw '' 'difftest' 'units' 'family' '' line 1000 1 '' '' ''\nDIMENSION x '' absolute 1 1\n"},
				{WaitFile: "go"},
				{Emit: "BEGIN difftest.tw\nSET x = 1\nEND\n"},
				{Hang: true},
			}}}},
			enable: []string{"difftesttwo"},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not define its chart", func(s [][]plugin.Record) bool {
					return len(s) >= 1 && plugin.Has(s[0], "waiting", "go")
				})
				time.Sleep(time.Second)
				two := installMore(t, p, morePlugin{dir: "plugins.d", file: "difftesttwo.plugin", sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
					{Emit: "CHART difftest.tw '' 'difftest' 'units' 'family' '' line 1000 1 '' '' ''\nDIMENSION x '' absolute 1 1\n"},
					{Hang: true},
				}}}}})
				waitPlugin(t, p, two, "the second plugin did not hang", startHangs(1))
				time.Sleep(time.Second)
				for i := range ls {
					if err := ls[i].Release("go"); err != nil {
						t.Fatal(err)
					}
				}
				waitPlugin(t, p, ls, "the first plugin did not end", startEnded(1))
				time.Sleep(2 * time.Second)
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				hasRecord(t, classes, "collector.log thread=PD[difftest]", "PLUGINSD: keyword BEGIN: 'host:parity-parent/chart:difftest.tw' is collected twice (my tid N, other collector tid N)")
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", "attempted to clear collector_tid N for 'host:parity-parent/chart:difftest.tw/' from non-owner thread N during THREAD CLEANUP")
			},
		},
		// killed by SIGUSR1: abnormal, disabled, never started again
		"crash": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Raise: "SIGUSR1"}}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not end", startEnded(1))
				time.Sleep(3 * time.Second)
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 1 {
					t.Errorf("oracle: %d starts", len(starts))
				}
				hasRecord(t, classes, "collector.log comm=spawn-plugins", "killed by signal 10")
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", "exited abnormally. Disabling it.")
			},
		},
		// a plugin that ignores QUIT, stdin's end and SIGTERM: the kill escalates to SIGKILL after 3 + 2 s, abnormal,
		// disabled
		"deaf": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{{IgnoreTerm: true}, {StayOnEOF: true}, {Collect: &plugin.Collect{Chart: "difftest.d", Dims: []string{"x"}, N: 1}},
					{Emit: "DIFFTEST_UNKNOWN\n"}, {Hang: true}}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not hang", startHangs(1))
				time.Sleep(10 * time.Second)
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 1 {
					t.Errorf("oracle: %d starts", len(starts))
				}
				hasRecord(t, classes, "collector.log comm=spawn-plugins", "killed by signal 9")
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", "exited abnormally. Disabling it.")
			},
		},
		// a plugin that stays after stdin's end but dies by SIGTERM: a clean exit after the 3 s grace, started again
		"term": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{{StayOnEOF: true}, {Collect: &plugin.Collect{Chart: "difftest.t", Dims: []string{"x"}, N: 1}},
					{Emit: "DIFFTEST_UNKNOWN\n"}, {Hang: true}}},
				{Steps: []plugin.Step{{Hang: true}}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				starts := waitPlugin(t, p, ls, "no second start", startHangs(2))
				checkRestart(t, starts, 2, time.Second, 2500*time.Millisecond, "one update every after the SIGTERM")
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 2 || plugin.ViewOf(starts[0]).End != "signal SIGTERM" {
					t.Errorf("oracle: %d starts, the first ended %q", len(starts), plugin.ViewOf(starts[0]).End)
				}
			},
		},
	}
	runPluginCases(t, cases)
}

// pluginCallIDRe is a call's transaction where the agent writes it to a plugin (pluginsd_functions.c:24-50, :255-257,
// :323-327): an id the agent made itself (a DynCfg echo's, dyncfg-echo.c:94-107, nrpc-calls.c:657-660) differs run to
// run; one a case sent (fnTxPrefix) is kept.
var pluginCallIDRe = regexp.MustCompile(`(?m)^FUNCTION(?:_PAYLOAD|_CANCEL|_PROGRESS)? [0-9a-f]{32}\b`)

// maskedViews are views with the agent's own call ids on stdin masked RANDOM (pluginCallIDRe).
func maskedViews(vs []plugin.View) []plugin.View {
	out := slices.Clone(vs)
	for i := range out {
		out[i].Stdin = pluginCallIDRe.ReplaceAllStringFunc(out[i].Stdin, func(m string) string {
			if id := m[len(m)-32:]; !strings.HasPrefix(id, fnTxPrefix) {
				return m[:len(m)-32] + "RANDOM"
			}
			return m
		})
	}
	return out
}

// runPluginCases plays each case on a pair (the fake plugin installed in each run directory), then stops both and
// compares the starts' views (maskedViews), the other fake plugins' and the log classes, after the oracle's guards; a
// case's after hook comes last.
func runPluginCases(t *testing.T, cases map[string]pluginCase) {
	bins := binaries(t)
	engine, err := plugin.Engine()
	if err != nil {
		t.Fatal(err)
	}
	for _, name := range slices.Sorted(maps.Keys(cases)) {
		c := cases[name]
		t.Run(name, func(t *testing.T) {
			var ls [2]plugin.Layout
			side := 0
			ue := max(c.ue, 1)
			opts := pluginsOptions(ue, c.dirs, c.enable, c.logs)
			if c.adjust != nil {
				c.adjust(&opts)
			}
			p := startPairWith(t, opts, parentIdentity, bins, [2]string{}, [2]Role{Oracle, Candidate},
				func(t *testing.T, runDir string) {
					if c.prepare != nil {
						c.prepare(t, runDir)
					}
					l, err := plugin.Install(runDir, engine, c.sc)
					if err != nil {
						t.Fatal(err)
					}
					ls[side] = l
					for _, m := range c.more {
						if _, err := plugin.InstallAs(runDir, filepath.Join(runDir, m.dir), m.file, engine, m.sc); err != nil {
							t.Fatal(err)
						}
					}
					side++
				})
			c.play(t, p, ls)
			var views [2][]plugin.View
			var classes [2]map[string][]string
			var oracleStarts [][]plugin.Record
			// every other fake plugin's start views (installed before the start or during play), by file
			var more [2]map[string][]plugin.View
			for i, side := range p.Each() {
				// the stop's QUIT, SIGTERM and SIGKILL come from the cancelled plugin thread at once, so what the plugin
				// records of them is a race: each start's view ends at the stop (the kill shows in the spawn server's
				// records); the daemon reaps after the plugin is gone
				stopAt := time.Now()
				if err := side.Daemon.Stop(); err != nil {
					t.Errorf("%s: stop: %v", side.Role, err)
				}
				starts, err := ls[i].Starts()
				if err != nil {
					t.Fatal(err)
				}
				for _, s := range starts {
					views[i] = append(views[i], plugin.ViewOf(plugin.Before(s, stopAt)))
				}
				dirs, err := filepath.Glob(filepath.Join(side.Daemon.Opts.RunDir, "engines", "*"))
				if err != nil {
					t.Fatal(err)
				}
				more[i] = map[string][]plugin.View{}
				for _, dir := range dirs {
					starts, err := plugin.Layout{Dir: dir}.Starts()
					if err != nil {
						t.Fatal(err)
					}
					more[i][filepath.Base(dir)] = nil
					for _, s := range starts {
						more[i][filepath.Base(dir)] = append(more[i][filepath.Base(dir)], plugin.ViewOf(plugin.Before(s, stopAt)))
					}
				}
				if i == 0 {
					oracleStarts = starts
				}
				classes[i] = pluginLogClasses(t, side.Daemon)
				if others := otherPluginThreads(t, side.Daemon); len(others) > 0 {
					t.Errorf("%s: installed plugins ran: %v", side.Role, others)
				}
			}
			if len(views[0]) == 0 || !slices.Equal(views[0][0].Args, []string{fmt.Sprint(ue), "alpha", "b c"}) {
				t.Errorf("oracle: the first start's arguments: %+v", views[0])
			}
			c.guard(t, oracleStarts, classes[0])
			if c.mask != nil {
				for i := range classes {
					c.mask(classes[i])
				}
			}
			for i := range views {
				views[i] = maskedViews(views[i])
				if c.views != nil {
					c.views(views[i])
				}
				for k, v := range more[i] {
					more[i][k] = maskedViews(v)
				}
			}
			if fmt.Sprintf("%+v", views[0]) != fmt.Sprintf("%+v", views[1]) {
				t.Errorf("starts differ\noracle:    %+v\ncandidate: %+v", views[0], views[1])
			}
			if fmt.Sprintf("%+v", more[0]) != fmt.Sprintf("%+v", more[1]) {
				t.Errorf("the other fake plugins' starts differ\noracle:    %+v\ncandidate: %+v", more[0], more[1])
			}
			for _, class := range slices.Sorted(maps.Keys(classes[0])) {
				diffLines(t, class, classes[0][class], classes[1][class])
				t.Logf("oracle %s:\n%s", class, strings.Join(classes[0][class], "\n"))
			}
			for class := range classes[1] {
				if _, ok := classes[0][class]; !ok {
					t.Errorf("candidate only: %s: %q", class, classes[1][class])
				}
			}
			if c.after != nil {
				c.after(t, p)
			}
		})
	}
}

// TestPluginsLifecycle (check `plugins.lifecycle`, PARITY_LONG; M8 commit 3, D142): the fake plugin over minutes:
// C's threshold of ten runs without data, the 120 s read timeout, and PLUGIN_KEEPALIVE holding it off.
func TestPluginsLifecycle(t *testing.T) {
	if os.Getenv("PARITY_LONG") != "1" {
		t.Skip("PARITY_LONG unset (each case runs for about two minutes)")
	}
	hang := plugin.Step{Hang: true}
	runPluginCases(t, map[string]pluginCase{
		// every start exits 0 without data: ten starts ten update everies apart, the eleventh disables the plugin
		"threshold": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Exit: plugin.ExitCode(0)}}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				for i, side := range p.Each() {
					if starts, ok := ls[i].WaitFor(150*time.Second, startEnded(11)); !ok {
						t.Fatalf("%s: 11 starts did not end within 150 s: %d", side.Role, len(starts))
					}
				}
				// one more wait: a twelfth start would come ten update everies later
				time.Sleep(12 * time.Second)
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 11 {
					t.Errorf("oracle: %d starts", len(starts))
				}
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", "PLUGINSD: 'host:'parity-parent', '<RUN>/plugins.d/difftest.plugin' (pid P) does not generate useful output, although it reports success (exits with 0).We have tried to collect something 11 times - unsuccessfully. Disabling it.")
			},
		},
		// silent for over 120 s after a collection: the read times out, QUIT, a start again after one update every
		"timeout": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{{Collect: &plugin.Collect{Chart: "difftest.l", Dims: []string{"x"}, N: 1}}, hang}},
				{Steps: []plugin.Step{hang}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				for i, side := range p.Each() {
					if starts, ok := ls[i].WaitFor(150*time.Second, startHangs(2)); !ok {
						t.Fatalf("%s: no second start within 150 s: %d", side.Role, len(starts))
					}
				}
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 2 || plugin.ViewOf(starts[0]).Stdin != "QUIT" {
					t.Errorf("oracle: %d starts, %+v", len(starts), starts)
				}
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", `errno="110, Connection timed out"`)
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", "PARSER: timeout while waiting for data.")
				hasRecord(t, classes, "collector.log thread=PD[difftest]", "PLUGINSD: buffered reader not OK (-7)")
			},
		},
		// PLUGIN_KEEPALIVE every 60 s: no timeout over 130 s
		"keepalive": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				{Emit: "PLUGIN_KEEPALIVE\n"}, {SleepMs: 60000}, {Emit: "PLUGIN_KEEPALIVE\n"}, {SleepMs: 60000},
				{Emit: "PLUGIN_KEEPALIVE\n"}, hang,
			}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not start", func(s [][]plugin.Record) bool { return len(s) >= 1 })
				time.Sleep(130 * time.Second)
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 1 {
					t.Errorf("oracle: %d starts", len(starts))
				}
			},
		},
	})
}
