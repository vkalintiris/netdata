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
// the build info as in every other check; nothing installed there runs) and the run directory's second, a scan every
// second, `[plugin:difftest]` with an update every of 1 and two options (dash splits them to `alpha` and `b c`).
func pluginsOptions() daemon.Options {
	stock := filepath.Join(filepath.Dir(os.Getenv("PARITY_ORACLE")), "..", "libexec", "netdata", "plugins.d")
	return daemon.Options{
		StreamMemoryMode: "ram",
		StorageTiers:     1,
		PulseOff:         true,
		PluginsDir:       fmt.Sprintf("%q %q", filepath.Clean(stock), "{run}/plugins.d"),
		PluginsExtra:     "    " + plugin.Name + " = yes\n    check for new plugins every = 1\n",
		ConfExtra:        "[plugin:" + plugin.Name + "]\n    update every = 1\n    command options = alpha 'b c'\n",
	}
}

var (
	pidRe     = regexp.MustCompile(`\bpid \d+`)
	requestRe = regexp.MustCompile(`\brequest \d+`)
)

// pluginLogClasses are a daemon's records about the fake plugin, per class, each in file order (several processes
// write the collectors log): its thread's in daemon.log, the plugins thread's naming it, its thread's in
// collector.log, the spawn server's naming its file, and the raw lines it wrote to stderr. Pids and spawn request
// numbers are masked (C counts every spawn, system-info.sh's included).
func pluginLogClasses(t *testing.T, d *daemon.Daemon) map[string][]string {
	t.Helper()
	thread := "thread=PD[" + plugin.Name + "]"
	classes := map[string][]string{}
	add := func(class, line string) {
		line = normalizeLog(line, d.Opts.RunDir, "")
		line = pidRe.ReplaceAllString(line, "pid P")
		classes[class] = append(classes[class], requestRe.ReplaceAllString(line, "request R"))
	}
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		switch {
		case strings.Contains(l, thread):
			add("daemon.log "+thread, l)
		case strings.Contains(l, "thread=PLUGINSD") && strings.Contains(l, plugin.Name):
			add("daemon.log thread=PLUGINSD", l)
		}
	}
	for _, l := range logLines(t, d.Opts.RunDir, "collector.log") {
		switch {
		case strings.Contains(l, thread):
			add("collector.log "+thread, l)
		case strings.Contains(l, "comm=spawn-plugins") && strings.Contains(l, plugin.Name+".plugin"):
			add("collector.log comm=spawn-plugins", l)
		case !strings.HasPrefix(l, "time="):
			add("collector.log raw", l)
		}
	}
	return classes
}

// otherPluginThreads are the plugin threads other than the fake plugin's in a daemon's logs: none may run.
func otherPluginThreads(t *testing.T, d *daemon.Daemon) []string {
	var out []string
	for _, name := range []string{"daemon.log", "collector.log"} {
		for _, l := range logLines(t, d.Opts.RunDir, name) {
			if th := threadOf(l); strings.HasPrefix(th, "PD[") && th != "PD["+plugin.Name+"]" && !slices.Contains(out, th) {
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
	// play waits for each side's plugin and compares what is visible while it runs, with the oracle's guards
	play func(t *testing.T, p *Pair, ls [2]plugin.Layout)
	// guard checks the oracle's records and log classes after the stop
	guard func(t *testing.T, starts [][]plugin.Record, classes map[string][]string)
}

// TestPluginsFakePlugin (check `plugins.fake-plugin`, M8 commit 0, D134): both agents run `difftest.plugin`
// (package plugin): a dash wrapper exec'ing a Go engine that plays a scenario and records its argv, its stdin and how
// it ended. Per case: what the agents show while it runs, then (both stopped) each start's view, the number of starts
// and the plugin's log records in five classes. Every guard runs on the oracle, so no case passes with two sides that
// never ran the plugin; neither side may start an installed plugin. Against a candidate the cases skip unless
// PARITY_PLUGINS=1 (no plugins.d orchestration in Rust before M8's orchestration commit).
func TestPluginsFakePlugin(t *testing.T) {
	bins := binaries(t)
	engine, err := plugin.Engine()
	if err != nil {
		t.Fatal(err)
	}
	cases := map[string]pluginCase{
		// five collections, held until released, then exit 0: the chart while held, gone after; a restart after one
		// update every; the second start killed by the agent's stop
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
						return len(s) == 1 && plugin.Has(s[0], "waiting", "release-1")
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
				var gaps [2]time.Duration
				for i, side := range p.Each() {
					if err := ls[i].Release("release-1"); err != nil {
						t.Fatal(err)
					}
					starts, ok := ls[i].WaitFor(30*time.Second, func(s [][]plugin.Record) bool {
						return len(s) == 2 && plugin.Has(s[1], "step", "hang")
					})
					if !ok {
						t.Fatalf("%s: no second start within 30 s of the exit", side.Role)
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
				// the agent's stop kills the hanging second start
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
						return len(s) == 1 && plugin.Has(s[0], "end", "")
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
					"collector.log comm=spawn-plugins": "difftest.plugin",
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
						return len(s) == 2 && plugin.Has(s[1], "step", "hang")
					})
					if !ok {
						t.Fatalf("%s: no second start within 30 s", side.Role)
					}
					gaps[i] = plugin.Time(starts[1], "start").Sub(plugin.Time(starts[0], "end"))
				}
				t.Logf("restarts after the bad line's stop: oracle %v, candidate %v", gaps[0], gaps[1])
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
	}
	for _, name := range slices.Sorted(maps.Keys(cases)) {
		c := cases[name]
		t.Run(name, func(t *testing.T) {
			if bins[0] != bins[1] && os.Getenv("PARITY_PLUGINS") != "1" {
				t.Skip("PARITY_PLUGINS=1 runs the fake plugin under the candidate (no plugins.d orchestration in Rust yet)")
			}
			var ls [2]plugin.Layout
			side := 0
			p := startPairWith(t, pluginsOptions(), parentIdentity, bins, [2]string{}, [2]Role{Oracle, Candidate},
				func(t *testing.T, runDir string) {
					l, err := plugin.Install(runDir, engine, c.sc)
					if err != nil {
						t.Fatal(err)
					}
					ls[side] = l
					side++
				})
			c.play(t, p, ls)
			var views [2][]plugin.View
			var classes [2]map[string][]string
			var oracleStarts [][]plugin.Record
			for i, side := range p.Each() {
				if err := side.Daemon.Stop(); err != nil {
					t.Errorf("%s: stop: %v", side.Role, err)
				}
				// the spawn server stops the plugin after the daemon exits (a hanging one by SIGKILL, recording nothing):
				// its last records are written once its process is gone
				starts, _ := ls[i].WaitFor(5*time.Second, func(s [][]plugin.Record) bool {
					if len(s) == 0 || len(s[len(s)-1]) == 0 {
						return true
					}
					_, err := os.Stat(fmt.Sprintf("/proc/%d", s[len(s)-1][0].Pid))
					return os.IsNotExist(err)
				})
				for _, s := range starts {
					views[i] = append(views[i], plugin.ViewOf(s))
				}
				if i == 0 {
					oracleStarts = starts
				}
				classes[i] = pluginLogClasses(t, side.Daemon)
				if others := otherPluginThreads(t, side.Daemon); len(others) > 0 {
					t.Errorf("%s: installed plugins ran: %v", side.Role, others)
				}
			}
			if len(views[0]) == 0 || !slices.Equal(views[0][0].Args, []string{"1", "alpha", "b c"}) {
				t.Errorf("oracle: the first start's arguments: %+v", views[0])
			}
			c.guard(t, oracleStarts, classes[0])
			if fmt.Sprintf("%+v", views[0]) != fmt.Sprintf("%+v", views[1]) {
				t.Errorf("starts differ\noracle:    %+v\ncandidate: %+v", views[0], views[1])
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
		})
	}
}
