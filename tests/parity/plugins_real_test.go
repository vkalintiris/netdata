// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"io/fs"
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
)

// The real plugins (M8 commit 10, D181; plan evidence/2026-10-03-plan-m8-commit10.md §5): `plugins.real-set` runs the
// oracle's installed plugins.d as installed (daemon.Options.PluginsStock: `enable running new plugins = yes`, the
// internal collectors off) under both agents at once, unprivileged, and compares what each agent makes of them: which
// plugins end and how, the plugin threads, the configuration dump, each long-runner's context, the real Functions and
// DynCfg nodes, and the charts. What a plugin itself does on this host is the oracle's, taken at run time: the guards
// only demand that the oracle saw it and that both sides agree.

// realPluginsDir is the oracle's installed plugins directory: both agents' compiled-in default (the candidate is built
// with the oracle's paths), so PluginsDir stays empty.
func realPluginsDir() string {
	return filepath.Clean(filepath.Join(filepath.Dir(os.Getenv("PARITY_ORACLE")), "..", "libexec", "netdata", "plugins.d"))
}

// realSuffixRe is C's plugin file rule (plugins_d.c:247-278): a name ending in `.plugin`, `_plugin` or `-plugin`; the
// [plugins] name is the file's without it.
var realSuffixRe = regexp.MustCompile(`^(.+)[._-]plugin$`)

// realInstalled are the [plugins] names of the installed plugins, sorted: every file C's scan would start (no
// executable check, plugins_d.c:247-278), `otel-signal-viewer` skipped (:282-295).
func realInstalled(t *testing.T) []string {
	t.Helper()
	entries, err := os.ReadDir(realPluginsDir())
	if err != nil {
		t.Fatalf("parity: the oracle's plugins: %v", err)
	}
	var out []string
	for _, e := range entries {
		if m := realSuffixRe.FindStringSubmatch(e.Name()); m != nil && !e.IsDir() && m[1] != "otel-signal-viewer" {
			out = append(out, m[1])
		}
	}
	slices.Sort(out)
	return out
}

// realOtelEnv keeps each agent's otel-plugin off the stock endpoint (127.0.0.1:4317, which only one could bind) and out
// of the stock base directory inside the oracle's install tree (otel.yaml's `base_dir`): an ephemeral port and the run
// directory (otel-plugin config/env.rs: NETDATA_OTEL_CFG_ENDPOINT_PATH, NETDATA_OTEL_CFG_BASE_DIR; D181 E3).
var realOtelEnv = []string{"NETDATA_OTEL_CFG_ENDPOINT_PATH=127.0.0.1:0", "NETDATA_OTEL_CFG_BASE_DIR={run}/otel"}

// realCase is one plugins.real-set scenario.
type realCase struct {
	// only are the [plugins] names that run (every installed one when empty: the others get `<name> = no`)
	only []string
	// env is more daemon environment (after realOtelEnv)
	env []string
	// prepare lays out more files in each run directory before its daemon starts (tokens are always written)
	prepare func(t *testing.T, runDir string)
	// running are the plugins whose threads the oracle must show after the settle (every one in only when nil)
	running []string
	// play runs on each side at once (t.Errorf only) and returns its observations; pair, when set, plays instead, on
	// the test goroutine with both sides (agent restarts)
	play func(t *testing.T, x *dcSide) []string
	pair func(t *testing.T, p *Pair, xs [2]*dcSide) [2][]string
	// before runs on the running pair once both settled, before the play; compare once both played (dumps, lists,
	// charts compared side by side)
	before  func(t *testing.T, p *Pair)
	compare func(t *testing.T, p *Pair)
	// want are texts the oracle's observations or records must hold, wantNot texts none may hold
	want, wantNot []string
	// mask renders each side's observations before they are compared, records each access record: C's run-to-run
	// variation only
	mask    func(obs []string) []string
	records func(l string) string
	// after runs once both stopped and the records were compared
	after func(t *testing.T, p *Pair)
}

// runRealCases plays each case on a pair running the installed plugins: both sides settle (realSettle), their plugin
// threads are compared, the case plays on both at once and its observations are compared, then both stop at once and
// their records are compared (realOutcomes, realOwnRecords), with the oracle's guards: the case's texts, the report
// guard and the oracle's install tree untouched.
func runRealCases(t *testing.T, cases map[string]realCase) {
	bins := binaries(t)
	installed := realInstalled(t)
	if len(installed) == 0 {
		t.Skipf("no plugin installed in %s", realPluginsDir())
	}
	tree := realTreeOf(t)
	for _, name := range slices.Sorted(maps.Keys(cases)) {
		c := cases[name]
		t.Run(name, func(t *testing.T) {
			guardSeen := reportGuardSeen()
			opts := daemon.Options{DBMode: "ram", StreamMemoryMode: "ram", StorageTiers: 1, PulseOff: true,
				PluginsStock: true, Env: append(slices.Clone(realOtelEnv), c.env...)}
			for _, n := range installed {
				if len(c.only) > 0 && !slices.Contains(c.only, n) {
					opts.PluginsExtra += "    " + n + " = no\n"
				}
			}
			p := startPairWith(t, opts, parentIdentity, bins, [2]string{}, [2]Role{Oracle, Candidate},
				func(t *testing.T, runDir string) {
					fnWriteTokens(t, runDir)
					if c.prepare != nil {
						c.prepare(t, runDir)
					}
				})
			var xs [2]*dcSide
			for i, side := range p.Each() {
				xs[i] = &dcSide{fnHTTPSide{role: side.Role, d: side.Daemon}}
			}
			threads := dcBoth(xs, func(_ int, x *dcSide) []string {
				th := realSettle(t, x.d, realSettleLeast)
				exit0 := "none"
				if at := realLastExit0(t, x.d); !at.IsZero() {
					exit0 = fmt.Sprintf("%.1f s", at.Sub(x.d.LaunchStartedAt).Seconds())
				}
				t.Logf("%s: settled %.1f s after its launch, the last exit 0 without data at %s", x.role,
					time.Since(x.d.LaunchStartedAt).Seconds(), exit0)
				return th
			})
			running := c.running
			if running == nil {
				running = c.only
			}
			for _, n := range running {
				if !slices.Contains(threads[0], realThreadTag(n)) {
					t.Errorf("oracle: no thread %s after the settle: %v", realThreadTag(n), threads[0])
				}
			}
			diffLines(t, "plugin threads after the settle", threads[0], threads[1])
			t.Logf("oracle plugin threads: %v", threads[0])

			if c.before != nil {
				c.before(t, p)
			}
			var obs [2][]string
			var played [2]bool
			if c.pair != nil {
				obs, played = c.pair(t, p, xs), [2]bool{true, true}
			} else if c.play != nil {
				played = dcBoth(xs, func(i int, x *dcSide) bool {
					obs[i] = c.play(t, x)
					return true
				})
			}
			if (c.play != nil || c.pair != nil) && !played[0] {
				t.Fatal("oracle: the case did not play")
			}
			now := time.Now()
			for i, side := range p.Each() {
				obs[i] = realRunMask(obs[i], side.Daemon.Opts.RunDir)
				obs[i] = dcClock(obs[i], now.Add(-10*time.Minute), now.Add(time.Minute))
				if c.mask != nil {
					obs[i] = c.mask(obs[i])
				}
			}
			diffLines(t, "observations", obs[0], obs[1])
			t.Logf("oracle observations:\n%s", strings.Join(obs[0], "\n"))
			if c.compare != nil {
				c.compare(t, p)
			}

			stopAt := time.Now()
			stopBoth(t, p)
			var outcomes, own [2]map[string][]string
			var access, dyncfg [2][]string
			for i, side := range p.Each() {
				outcomes[i] = realOutcomes(t, side.Daemon, realStopsOf(side.Daemon, stopAt))
				own[i] = realOwnRecords(t, side.Daemon, threads[0])
				for _, l := range fnHTTPAccess(t, side.Daemon) {
					l = realAccessMask(l)
					if c.records != nil {
						l = c.records(l)
					}
					access[i] = append(access[i], l)
				}
				dyncfg[i] = realDcRecords(t, side.Daemon)
			}
			realCompareClasses(t, "plugin records", outcomes)
			realCompareClasses(t, "plugins' own records", own)
			diffLines(t, "access records", access[0], access[1])
			diffLines(t, "DynCfg records", dyncfg[0], dyncfg[1])
			hay := slices.Concat(obs[0], access[0], dyncfg[0])
			for _, m := range []map[string][]string{outcomes[0], own[0]} {
				for _, lines := range m {
					hay = append(hay, lines...)
				}
			}
			for _, w := range c.want {
				if !slices.ContainsFunc(hay, func(l string) bool { return strings.Contains(l, w) }) {
					t.Errorf("oracle: nothing holds %q", w)
				}
			}
			for _, w := range c.wantNot {
				if i := slices.IndexFunc(hay, func(l string) bool { return strings.Contains(l, w) }); i >= 0 {
					t.Errorf("oracle: %q holds %q", hay[i], w)
				}
			}
			if n := reportGuardSeen(); n != guardSeen {
				t.Errorf("%d HTTPS attempts reached the report guard during the case", n-guardSeen)
			}
			realTreeUnchanged(t, tree)
			if c.after != nil {
				c.after(t, p)
			}
		})
	}
}

// realLengthRe is a head's length inside a quoted exchange.
var realLengthRe = regexp.MustCompile(`Content-Length: \d+`)

// realRunMask replaces a side's run directory in its observations (a plugin names the files it read, as scripts.d's
// file jobs their source), and the length of an answer that held it.
func realRunMask(obs []string, runDir string) []string {
	out := make([]string, len(obs))
	for i, o := range obs {
		if strings.Contains(o, runDir) {
			o = realLengthRe.ReplaceAllString(strings.ReplaceAll(o, runDir, "<RUN>"), "Content-Length: N")
		}
		out[i] = o
	}
	return out
}

// realThreadTag is a plugin thread's name as /proc shows it: `PD[<name>]` cut to 15 bytes (plugins_d.c:333-336, the
// kernel's TASK_COMM_LEN).
func realThreadTag(name string) string {
	tag := "PD[" + name + "]"
	if len(tag) > 15 {
		tag = tag[:15]
	}
	return tag
}

// realSettleLeast is the least time after a side's launch before its plugin threads count as settled: the quick-fail
// plugins end within about 2 s.
const realSettleLeast = 14 * time.Second

// realSettle waits until a daemon's plugin threads (pdThreads) stay the same for 3 s, at least `least` after its launch
// and 12 s after its last record of a plugin that exited 0 without data (its thread lingers ten update everies before
// it ends, plugins_d.c:125-190), within 60 s, and returns them.
func realSettle(t *testing.T, d *daemon.Daemon, least time.Duration) []string {
	t.Helper()
	if wait := time.Until(d.LaunchStartedAt.Add(least)); wait > 0 {
		time.Sleep(wait)
	}
	deadline := time.Now().Add(60 * time.Second)
	last, since := pdThreads(t, d), time.Now()
	for time.Since(since) < 3*time.Second || time.Since(realLastExit0(t, d)) < 12*time.Second {
		if time.Now().After(deadline) {
			t.Errorf("%s: the plugin threads did not settle within 60 s: %v", d.Opts.Binary, last)
			break
		}
		time.Sleep(250 * time.Millisecond)
		if now := pdThreads(t, d); !slices.Equal(now, last) {
			last, since = now, time.Now()
		}
	}
	return last
}

// realLastExit0 is the time of a daemon's last record of a plugin run that exited 0 without data (zero when none).
func realLastExit0(t *testing.T, d *daemon.Daemon) time.Time {
	t.Helper()
	var at time.Time
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if !strings.Contains(l, "does not generate useful output") {
			continue
		}
		if m := recordTimeRe.FindStringSubmatch(l); m != nil {
			if when, err := time.Parse(time.RFC3339Nano, m[1]); err == nil && when.After(at) {
				at = when
			}
		}
	}
	return at
}

// realCompareClasses compares two sides' record classes: each class line by line, a class only one side has failing.
func realCompareClasses(t *testing.T, what string, classes [2]map[string][]string) {
	t.Helper()
	for _, class := range slices.Sorted(maps.Keys(classes[0])) {
		diffLines(t, what+": "+class, classes[0][class], classes[1][class])
		t.Logf("oracle %s: %s:\n%s", what, class, strings.Join(classes[0][class], "\n"))
	}
	for _, class := range slices.Sorted(maps.Keys(classes[1])) {
		if _, ok := classes[0][class]; !ok {
			t.Errorf("%s: candidate only: %s: %q", what, class, classes[1][class])
		}
	}
}

var (
	// realSuccessfulRe is a long-runner's count of collections at the stop, above 0: how many it made is the run's
	// length; a plugin that never collected logs 0 (pluginsd_parser.c:1483), compared as it is
	realSuccessfulRe = regexp.MustCompile(`disconnected after [1-9]\d* successful data collections`)
	// realParserChartRe is the chart a plugin thread's parser last wrote (its log stack's instance and context,
	// pluginsd_parser.c:1391-1408, :1437-1442), which a record written on that thread carries: which chart scripts.d
	// wrote last is timing (C vs C: DYNCFG USER ACTION records with it from 'enable' on in f1 and k80b, from 'disable'
	// on in k80, without it elsewhere; topology_test.go's chartFieldsRe); rendered as the two empty fields' spaces
	realParserChartRe = regexp.MustCompile(` instance=\S+ context=\S+`)
	// realParserNodeRe is the host the thread's parser last wrote for (its log stack's node, pluginsd_parser.c:1382-1389,
	// :1439): the vnode after a job's batch for it, localhost after the plugin's own (C vs C: the oracle's "Checking
	// virtual status" and "Reseting virtual host status" at the stop with node=parity-svnode in k80-r1, parity-parent in
	// every other run)
	realParserNodeRe = regexp.MustCompile(` node=\S+`)
	// realStopRaceRe are a running plugin's thread's records of its cancel (a stop's or an agent restart's) when it
	// found the thread waiting for input (pluginsd_parser.c:1440-1475): whether it was waiting or parsing a line is a
	// race (C vs C, a1: the systemd-journal thread logged them on one side, systemd-units' on the other)
	realStopRaceRe = regexp.MustCompile(`msg="(PARSER: thread cancelled while waiting for data\.|` +
		`PLUGINSD: buffered reader not OK \(-8\))"`)
	// realStopPairRe are a long-runner's thread's records of its kill at an agent's stop: whether the spawn server
	// reaped the killed plugin before the shutdown cancelled its wait (errno 125, "giving up waiting … after SIGKILL")
	// and how it died ("exited abnormally" when not by TERM or PIPE, plugins_d.c:61-120) are the stop walk's race
	// (D163): C vs C, the oracle's journal-dyncfg logged each twice (both sessions) in l1, once in real4
	realStopPairRe = regexp.MustCompile(`msg="(SPAWN PARENT: giving up waiting for pid P after SIGKILL \(request R\)|` +
		`PLUGINSD: 'host:[^']*', '[^']*' \(pid P\) exited abnormally\. Disabling it\.")`)
	// realQuitRaceRe is the QUIT a plugin thread writes once its plugin's run ended (a DISABLE), failing with EPIPE when
	// the plugin already exited: whether it did is a race (C vs C, a2: perf's on one side, ioping's on the other)
	realQuitRaceRe = regexp.MustCompile(`errno="32, Broken pipe" .*` +
		`msg="PLUGINSD: cannot send command to plugin \(fd = \d+, sent bytes = -1 out of 4\)"`)
	// realPyTimeRe is python.d's own time stamp on its stderr lines
	realPyTimeRe = regexp.MustCompile(`^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d: `)
	// realAccessSizeRe are the requests whose answers carry each side's run directory (/netdata.conf) or follow the
	// box's processes (apps.plugin's charts and contexts, the contexts' version in /api/v3/functions)
	realAccessSizeRe = regexp.MustCompile(` request="?(/netdata\.conf|/api/v1/charts|/api/v1/contexts|/api/v3/functions)"?$`)
	// realOwnTimeRe and realOwnTidRe are a plugin's own record's time and thread id; realRFC3339Re a raw line's leading
	// time (otel-plugin's tracing lines)
	realOwnTimeRe = regexp.MustCompile(`^time=\S+ `)
	realOwnTidRe  = regexp.MustCompile(` tid=\d+`)
	realRFC3339Re = regexp.MustCompile(`^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+Z`)
	realPidNumRe  = regexp.MustCompile(`\b(pid[= ]|PID )\d+`)
	realProcRe    = regexp.MustCompile(`/proc/\d+/`)
	realPortRe    = regexp.MustCompile(`127\.0\.0\.1:\d+`)
	realSockPid   = regexp.MustCompile(`-\d+\.sock\b`)
)

// realOutcomes are a daemon's records of its plugin threads, per class (pluginLogClassesOf over every `PD[` thread
// and the spawn server's records of the plugins directory), with C's races masked: the stop walk (maskStopWalk,
// D163), a long-runner's count of collections, the cancel's records (realStopRaceRe: a stop's or an agent restart's),
// the QUIT racing a plugin's exit (realQuitRaceRe), the chart and host the thread's parser last wrote
// (realParserState), a raw fragment written before a record (realSplitRecord), and in each of the agent's stops
// (`stops`: its restarts' and its final stop's) a plugin thread's kill records (realStopPairRe); the plugins' raw
// stderr goes to realOwnRecords.
func realOutcomes(t *testing.T, d *daemon.Daemon, stops []realStop) map[string][]string {
	t.Helper()
	pdThread := func(th string) bool { return strings.HasPrefix(th, "PD[") }
	classes := pluginLogClassesOf(t, d, time.Time{}, pdThread, realPluginsDir())
	delete(classes, "collector.log raw")
	// a class is one thread's records in file order, so the records written in a stop are those between the counts
	// written before its beginning and before its end
	inStop := map[string][]bool{}
	for class, lines := range classes {
		inStop[class] = make([]bool, len(lines))
	}
	for _, s := range stops {
		begin := pluginLogClassesOf(t, d, s.begin, pdThread, realPluginsDir())
		var end map[string][]string
		if !s.end.IsZero() {
			end = pluginLogClassesOf(t, d, s.end, pdThread, realPluginsDir())
		}
		for class, lines := range classes {
			to := len(lines)
			if end != nil {
				to = min(len(end[class]), to)
			}
			for i := len(begin[class]); i < to; i++ {
				inStop[class][i] = true
			}
		}
	}
	for class, lines := range classes {
		var kept []string
		for i, l := range lines {
			if realStopRaceRe.MatchString(l) || realQuitRaceRe.MatchString(l) ||
				(inStop[class][i] && strings.Contains(class, " thread=PD[") && realStopPairRe.MatchString(l)) {
				continue
			}
			l, _ = realSplitRecord(l)
			l = realParserState(l)
			kept = append(kept, realSuccessfulRe.ReplaceAllString(l, "disconnected after N successful data collections"))
		}
		classes[class] = kept
	}
	maskStopWalk(classes)
	return classes
}

// realStop is one of an agent's stops as the harness drove it: from the stop's beginning to the next launch's
// readiness (an agent restart's), or to the end (zero: the final stop's).
type realStop struct{ begin, end time.Time }

// realRestarts are the restarts a case made of each daemon (realRestart), for realOutcomes.
var realRestarts = struct {
	sync.Mutex
	of map[*daemon.Daemon][]realStop
}{of: map[*daemon.Daemon][]realStop{}}

// realRestart restarts a side's agent (dcSide.restart) and records the stop's window.
func realRestart(t *testing.T, x *dcSide) bool {
	t.Helper()
	begin := time.Now()
	ok := x.restart(t, "")
	realRestarts.Lock()
	realRestarts.of[x.d] = append(realRestarts.of[x.d], realStop{begin: begin, end: time.Now()})
	realRestarts.Unlock()
	return ok
}

// realStopsOf are a daemon's restarts (realRestart) and its final stop from `final` on.
func realStopsOf(d *daemon.Daemon, final time.Time) []realStop {
	realRestarts.Lock()
	defer realRestarts.Unlock()
	return append(slices.Clone(realRestarts.of[d]), realStop{begin: final})
}

// realParserState masks what a plugin thread's record carries of its parser's state: the chart and the host it last
// wrote (realParserChartRe, realParserNodeRe); other threads' records are left as they are.
func realParserState(l string) string {
	if !strings.Contains(l, " thread=PD[") {
		return l
	}
	l = realParserChartRe.ReplaceAllString(l, "  ")
	return realParserNodeRe.ReplaceAllString(l, " node=N")
}

// realDcRecords are dcRecords with a plugin thread's parser state masked (realParserState).
func realDcRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	out := dcRecords(t, d)
	for i, l := range out {
		out[i] = realParserState(l)
	}
	return out
}

// realSplitRecord cuts a record off the raw stderr fragment written before it on the same line, returning the record
// (its time masked) and the fragment: every plugin's stderr and the agent's collectors log share one file, and a
// plugin writing a line in pieces (libxen's "xencall: error: " then its message) can have a record land in between
// (C vs C, h2: PD[ioping]'s record after xenstat's "xencall: error: " on the oracle).
func realSplitRecord(l string) (record, fragment string) {
	if loc := realEmbeddedRecordRe.FindStringIndex(l); loc != nil && loc[0] > 0 {
		return realOwnTimeRe.ReplaceAllString(l[loc[0]:], "time=T "), l[:loc[0]]
	}
	return l, ""
}

// realEmbeddedRecordRe is a logfmt record's start (its time) inside a line.
var realEmbeddedRecordRe = regexp.MustCompile(`time=\d{4}-\d\d-\d\dT[0-9:.]+Z `)

// realAccessMask masks an access record's sizes when its answer carries the side's run directory or the box's live
// processes (realAccessSizeRe).
func realAccessMask(l string) string {
	if realAccessSizeRe.MatchString(l) {
		return responseBytesRe.ReplaceAllString(l, "${1}N")
	}
	return l
}

// realOwnRecords are what the quick-fail plugins themselves wrote to the collectors log (their stderr), per class: a
// plugin's logfmt record by its `comm=`, a raw line by its kind; each class a multiset (sorted: the plugins start at
// once), with the clocks, thread ids, pids and run directories masked. The long-runners' own records (those of a
// plugin with a thread among `running`, the oracle's after the settle: apps.plugin's per process, otel-plugin's
// tracing lines, scripts.d's job manager) follow the box and the run's timing, so they are left out (plan §5.1).
func realOwnRecords(t *testing.T, d *daemon.Daemon, running []string) map[string][]string {
	t.Helper()
	longRunner := func(plugin string) bool {
		return slices.ContainsFunc(running, func(tag string) bool {
			name := strings.TrimSuffix(strings.TrimPrefix(tag, "PD["), "]")
			return plugin != "" && strings.HasPrefix(plugin, name)
		})
	}
	classes := map[string][]string{}
	// a raw fragment a record cut off (realSplitRecord) is joined to the next raw line, the rest of its plugin's write
	pending := ""
	for _, l := range logLines(t, d.Opts.RunDir, "collector.log") {
		record, fragment := realSplitRecord(l)
		if fragment != "" {
			pending, l = pending+fragment, record
		} else if pending != "" && !strings.HasPrefix(l, "time=") {
			l, pending = pending+l, ""
		}
		comm := logField(l, "comm")
		if comm == "comm=netdata" || comm == "comm=spawn-plugins" {
			continue
		}
		class, plugin := comm, strings.TrimSuffix(strings.TrimPrefix(comm, "comm="), ".plugin")
		switch {
		case !strings.HasPrefix(l, "time="):
			class, plugin = "raw", ""
			if realRFC3339Re.MatchString(l) {
				class, plugin = "raw tracing", "otel"
			}
		case comm == "comm=?":
			class = logField(l, "plugin")
			plugin = strings.TrimPrefix(class, "plugin=")
		}
		if longRunner(plugin) {
			continue
		}
		l = realOwnTimeRe.ReplaceAllString(l, "time=T ")
		l = realRFC3339Re.ReplaceAllString(l, "T")
		l = realPyTimeRe.ReplaceAllString(l, "T: ")
		l = realOwnTidRe.ReplaceAllString(l, " tid=N")
		l = strings.ReplaceAll(l, d.Opts.RunDir, "<RUN>")
		l = shortRunRe.ReplaceAllString(l, "<RT>")
		l = realPidNumRe.ReplaceAllString(l, "${1}P")
		l = realProcRe.ReplaceAllString(l, "/proc/P/")
		l = realPortRe.ReplaceAllString(l, "127.0.0.1:PORT")
		l = realSockPid.ReplaceAllString(l, "-P.sock")
		classes[class] = append(classes[class], l)
	}
	for _, lines := range classes {
		slices.Sort(lines)
	}
	return classes
}

// realContexts are each running plugin's process as /proc shows it, by its file: its comm, argv, parent's comm,
// environment (sorted; each side's directories and invocation ids masked, a variable passed down from the harness
// unchanged shown as `<inherited>`, never its value) and the status lines a spawn sets (signals, capabilities, umask).
func realContexts(t *testing.T, d *daemon.Daemon) map[string][]string {
	t.Helper()
	server := spawnServerOf(d.PID())
	if server == 0 {
		t.Errorf("%s: no spawn-plugins child", d.Opts.Binary)
		return nil
	}
	mask := func(v string) string {
		v = strings.ReplaceAll(v, d.Opts.RunDir, "<RUN>")
		v = shortRunRe.ReplaceAllString(v, "<RT>")
		return v
	}
	out := map[string][]string{}
	entries, _ := os.ReadDir("/proc")
	for _, e := range entries {
		pid, err := strconv.Atoi(e.Name())
		if err != nil {
			continue
		}
		ppid, comm, _ := procStat(pid)
		if ppid != server {
			continue
		}
		cmdline, err := os.ReadFile(fmt.Sprintf("/proc/%d/cmdline", pid))
		argv := strings.Split(strings.TrimSuffix(string(cmdline), "\x00"), "\x00")
		if err != nil || !strings.HasPrefix(argv[0], realPluginsDir()+"/") {
			continue
		}
		_, parent, _ := procStat(ppid)
		view := []string{"comm " + comm, fmt.Sprintf("argv %q", argv), "parent " + parent}
		var env []string
		for _, kv := range envOf(pid) {
			k, v, _ := strings.Cut(kv, "=")
			if own, ok := os.LookupEnv(k); ok && own == v {
				// the harness's own, passed down: its value is not printed (the box's environment holds secrets)
				kv = k + "=<inherited>"
			}
			env = append(env, kv)
		}
		// each agent's own invocation id (NETDATA_INVOCATION_ID, INVOCATION_ID: spawnEnvMasks), on the raw entries
		for _, kv := range maskEnv(env) {
			view = append(view, "env "+mask(kv))
		}
		st := procStatusOf(t, pid)
		for _, k := range []string{"Umask", "SigBlk", "SigIgn", "CapInh", "CapPrm", "CapEff", "CapBnd", "CapAmb",
			"NoNewPrivs", "Seccomp"} {
			view = append(view, k+" "+st[k])
		}
		out[filepath.Base(argv[0])] = view
	}
	return out
}

// reportGuardSeen is how many attempts reached TestMain's report guard so far (without taking them: TestMain's own
// check still fails the run).
func reportGuardSeen() int {
	reportGuard.mu.Lock()
	defer reportGuard.mu.Unlock()
	return len(reportGuard.requests)
}

// realTree is the oracle's install tree as a listing: each entry's path, kind, mode, size and modification time.
type realTree struct {
	root    string
	entries []string
}

// realTreeOf lists the oracle's install tree (the stock otel would write into it, python.d its `__pycache__`).
func realTreeOf(t *testing.T) realTree {
	t.Helper()
	root := filepath.Clean(filepath.Join(filepath.Dir(os.Getenv("PARITY_ORACLE")), "..", ".."))
	tree := realTree{root: root}
	err := filepath.WalkDir(root, func(path string, e fs.DirEntry, err error) error {
		if err != nil {
			// an unreadable directory (the oracle's own 0750 trees are the user's): listed as such
			tree.entries = append(tree.entries, path+" unreadable")
			return nil
		}
		info, err := e.Info()
		if err != nil {
			tree.entries = append(tree.entries, path+" gone")
			return nil
		}
		tree.entries = append(tree.entries, fmt.Sprintf("%s %v %d %d", path, info.Mode(), info.Size(), info.ModTime().UnixNano()))
		return nil
	})
	if err != nil {
		t.Fatalf("parity: list %s: %v", root, err)
	}
	return tree
}

// realTreeUnchanged fails when an entry of the oracle's install tree changed, came or went since `before`.
func realTreeUnchanged(t *testing.T, before realTree) {
	t.Helper()
	after := realTreeOf(t)
	if changed := diffMultiset(before.entries, after.entries); len(changed) > 0 {
		t.Errorf("the oracle's install tree %s changed (plugins write into it):\n%s", before.root, strings.Join(changed, "\n"))
	}
}

// diffMultiset are the lines only one of a and b has, `-` for a's and `+` for b's.
func diffMultiset(a, b []string) []string {
	count := map[string]int{}
	for _, l := range a {
		count[l]--
	}
	for _, l := range b {
		count[l]++
	}
	var out []string
	for _, l := range slices.Sorted(maps.Keys(count)) {
		switch n := count[l]; {
		case n < 0:
			out = append(out, "- "+l)
		case n > 0:
			out = append(out, "+ "+l)
		}
	}
	return out
}

// realCharts are a daemon's chart definitions from `/api/v1/charts` of the charts keep holds for, by plugin (each
// plugin's charts in the answer's order: how the plugins' charts interleave is their start order's race, the
// Functions', C vs C a1), without the clock: duration, first and last entry.
func realCharts(t *testing.T, d *daemon.Daemon, keep func(id string, chart Value) bool) Value {
	t.Helper()
	return realChartsAt(t, d, "/api/v1/charts", keep)
}

// realChartsAt is realCharts from a target of `/api/v1/charts` (a poll's, tagged `harness=wait` so the access records
// leave it out).
func realChartsAt(t *testing.T, d *daemon.Daemon, target string, keep func(id string, chart Value) bool) Value {
	t.Helper()
	// t.Errorf only: the scripts case reads charts on each side's goroutine
	b, err := rawExchange(d.Addr, fnHTTPGet(target, ""), 30*time.Second)
	if err != nil {
		t.Errorf("%s: /api/v1/charts: %v", d.Opts.Binary, err)
		return Value{Kind: KindObject}
	}
	doc, err := ParseJSON(httpBody(b))
	if err != nil {
		t.Errorf("%s: /api/v1/charts: %v: %s", d.Opts.Binary, err, truncateBytes(b))
		return Value{Kind: KindObject}
	}
	charts, _, _, _ := fnMember(doc, "charts")
	byPlugin := map[string][]Member{}
	for _, m := range charts.Members {
		if !keep(m.Key, m.Value) {
			continue
		}
		plugin, _, _, _ := fnMember(m.Value, "plugin")
		byPlugin[plugin.Text] = append(byPlugin[plugin.Text], m)
	}
	out := Value{Kind: KindObject}
	for _, plugin := range slices.Sorted(maps.Keys(byPlugin)) {
		out.Members = append(out.Members, Member{Key: plugin, Value: Value{Kind: KindObject, Members: byPlugin[plugin]}})
	}
	return ApplyMasks(out, []Mask{
		{Pattern: "*.*.duration", Reason: "the clock"},
		{Pattern: "*.*.first_entry", Reason: "the clock"},
		{Pattern: "*.*.last_entry", Reason: "the clock"},
	})
}

// realCompareCharts compares both sides' realCharts, each chart's labels in any order (C keeps them in pointer
// order, D22.2), and the more `unordered` paths.
func realCompareCharts(t *testing.T, p *Pair, what string, keep func(id string, chart Value) bool, unordered ...string) {
	t.Helper()
	var v [2]Value
	for i, side := range p.Each() {
		v[i] = realCharts(t, side.Daemon, keep)
	}
	if len(v[0].Members) == 0 {
		t.Errorf("oracle: no %s", what)
	}
	for _, d := range Compare(v[0], v[1], append([]string{"*.*.chart_labels"}, unordered...)...) {
		t.Errorf("%s: %s", what, d)
	}
	var n int
	for _, m := range v[0].Members {
		n += len(m.Value.Members)
	}
	t.Logf("compared %d %s of %d plugins", n, what, len(v[0].Members))
}

// realCompareContexts compares both sides' `/api/v1/contexts` of the contexts keep holds for, without the clock, by
// the plugin whose charts make each (`/api/v1/charts`), each plugin's contexts in the answer's order: the plugins start
// at once, so how their contexts interleave is the start order's race (the Functions', C vs C a1).
func realCompareContexts(t *testing.T, p *Pair, keep func(id string) bool) {
	t.Helper()
	var v [2]Value
	for i, side := range p.Each() {
		owner := map[string]string{}
		for _, g := range realCharts(t, side.Daemon, func(string, Value) bool { return true }).Members {
			for _, c := range g.Value.Members {
				ctx, _, _, _ := fnMember(c.Value, "context")
				owner[ctx.Text] = g.Key
			}
		}
		b, err := rawExchange(side.Daemon.Addr, fnHTTPGet("/api/v1/contexts", ""), 30*time.Second)
		if err != nil {
			t.Fatalf("%s: /api/v1/contexts: %v", side.Role, err)
		}
		doc, err := ParseJSON(httpBody(b))
		if err != nil {
			t.Fatalf("%s: /api/v1/contexts: %v", side.Role, err)
		}
		ctx, _, _, _ := fnMember(doc, "contexts")
		byPlugin := map[string][]Member{}
		for _, m := range ctx.Members {
			if keep(m.Key) {
				byPlugin[owner[m.Key]] = append(byPlugin[owner[m.Key]], m)
			}
		}
		grouped := Value{Kind: KindObject}
		for _, plugin := range slices.Sorted(maps.Keys(byPlugin)) {
			grouped.Members = append(grouped.Members, Member{Key: plugin, Value: Value{Kind: KindObject, Members: byPlugin[plugin]}})
		}
		v[i] = ApplyMasks(grouped, []Mask{
			{Pattern: "*.*.first_time_t", Reason: "the clock"},
			{Pattern: "*.*.last_time_t", Reason: "the clock"},
		})
	}
	n := 0
	for _, g := range v[0].Members {
		n += len(g.Value.Members)
	}
	if n == 0 {
		t.Error("oracle: no context kept")
	}
	for _, d := range Compare(v[0], v[1]) {
		t.Errorf("/api/v1/contexts: %s", d)
	}
	t.Logf("compared %d contexts of %d plugins", n, len(v[0].Members))
}

// realNotAppsContext holds for a context apps.plugin does not make (apps_output.c: `app.`, `user.`, `usergroup.` and
// system.processes_state).
func realNotAppsContext(id string) bool {
	return !strings.HasPrefix(id, "app.") && !strings.HasPrefix(id, "user.") && !strings.HasPrefix(id, "usergroup.") &&
		id != "system.processes_state"
}

// realNotApps holds for a chart apps.plugin does not make: apps.plugin's follow every process of the box (the `apps`
// case compares those of the targets live on both sides).
func realNotApps(_ string, chart Value) bool {
	plugin, _, _, _ := fnMember(chart, "plugin")
	return plugin.Text != "apps.plugin"
}

// realCompareFunctions compares the lists of Functions on both sides: `/api/v1/functions` (head with the clock masked),
// `/api/v1/info`'s `functions` and `/api/v3/functions` (infoV2Volatile's clocks and durations masked, and the
// contexts' version: apps.plugin's charts follow the box's processes). The plugins' methods are compared in any
// order: the registry keeps them in arrival order and the plugins start at once (C vs C, a1: systemd-journal's and
// systemd-units' swapped); C's built-ins, registered at rrd_init before any plugin (rrd.c:185-190), must lead on both
// sides in their order.
func realCompareFunctions(t *testing.T, p *Pair) {
	t.Helper()
	var head, head3 [2]string
	var lists, infos, v3 [2]Value
	for i, side := range p.Each() {
		get := func(target string) ([]byte, Value) {
			b, err := rawExchange(side.Daemon.Addr, fnHTTPGet(target, ""), fnWait)
			if err != nil {
				t.Fatalf("%s: %s: %v", side.Role, target, err)
			}
			v, err := ParseJSON(httpBody(b))
			if err != nil {
				t.Fatalf("%s: %s: %v: %s", side.Role, target, err, truncateBytes(b))
			}
			return b, v
		}
		b, list := get("/api/v1/functions")
		head[i], lists[i] = fnCatalogHead(b), list
		_, infos[i] = get("/api/v1/info")
		b3, v := get("/api/v3/functions")
		v3[i] = ApplyMasks(v, append(slices.Clone(infoV2Volatile),
			Mask{Pattern: "versions.contexts_hard_hash", Reason: "apps.plugin's charts follow the box's processes"}))
		h3, _, _ := bytes.Cut(b3, []byte("\r\n\r\n"))
		head3[i] = string(contentLengthRe.ReplaceAll(maskRaw(h3), []byte("Content-Length: <masked>")))
		listed, _, _, _ := fnMember(list, "functions")
		keys := memberKeys(listed)
		if len(keys) < len(fnBuiltinNames) || !slices.Equal(keys[:len(fnBuiltinNames)], fnBuiltinNames) {
			t.Errorf("%s: /api/v1/functions does not open with C's built-ins %v: %v", side.Role, fnBuiltinNames, keys)
		}
	}
	// the `functions` member, keyed by its place among the members both sides write (fnCompareInfoFunctionsAt's rule:
	// a member the candidate does not write yet is left out of the place, D84.2)
	var placed [2]Value
	for i, side := range p.Each() {
		fns, before, after, ok := fnMember(fnCommonMembers(infos[i], infos[1-i]), "functions")
		if !ok {
			t.Errorf("%s: /api/v1/info has no functions member", side.Role)
		}
		placed[i] = Value{Kind: KindObject, Members: []Member{{Key: before + " < functions < " + after, Value: fns}}}
	}
	if head[0] != head[1] {
		t.Errorf("/api/v1/functions: heads differ\noracle:    %q\ncandidate: %q", head[0], head[1])
	}
	if head3[0] != head3[1] {
		t.Errorf("/api/v3/functions: heads differ\noracle:    %q\ncandidate: %q", head3[0], head3[1])
	}
	for _, d := range Compare(lists[0], lists[1], "functions") {
		t.Errorf("/api/v1/functions: %s", d)
	}
	for _, d := range Compare(placed[0], placed[1], "*") {
		t.Errorf("/api/v1/info functions: %s", d)
	}
	for _, d := range Compare(v3[0], v3[1], "functions[]") {
		t.Errorf("/api/v3/functions: %s", d)
	}
	t.Logf("oracle /api/v1/functions: %s\noracle /api/v3/functions: %s", lists[0], v3[0])
}

// realTreeNodeRe is a node's id in a DynCfg tree (quoted in an exchange), and realTreeTimesRe its times.
var (
	realTreeNodeRe  = regexp.MustCompile(`\\"([^"\\]+:[^"\\]*)\\":\{\\"type\\":`)
	realTreeTimesRe = regexp.MustCompile(`\\"(created_ut|modified_ut)\\":(\d{16})`)
)

// realRankPerPlugin renders a quoted tree exchange's node times as their ranks among the times of the nodes of the
// same plugin (the id up to its first `:`), `<plugin>-T<k>`: each plugin registers its nodes on its own thread
// (dyncfg.c:43-53) and the plugins start at once, so the order between two plugins' nodes is the start-order race
// (the Functions', C vs C a1); within a plugin it is compared.
func realRankPerPlugin(o string) string {
	nodes := realTreeNodeRe.FindAllStringSubmatchIndex(o, -1)
	pluginAt := func(pos int) string {
		plugin := ""
		for _, n := range nodes {
			if n[0] > pos {
				break
			}
			plugin, _, _ = strings.Cut(o[n[2]:n[3]], ":")
		}
		return plugin
	}
	values := map[string][]int64{}
	for _, m := range realTreeTimesRe.FindAllStringSubmatchIndex(o, -1) {
		v, _ := strconv.ParseInt(o[m[4]:m[5]], 10, 64)
		values[pluginAt(m[0])] = append(values[pluginAt(m[0])], v)
	}
	for plugin, vs := range values {
		slices.Sort(vs)
		values[plugin] = slices.Compact(vs)
	}
	var b strings.Builder
	last := 0
	for _, m := range realTreeTimesRe.FindAllStringSubmatchIndex(o, -1) {
		plugin := pluginAt(m[0])
		v, _ := strconv.ParseInt(o[m[4]:m[5]], 10, 64)
		k, _ := slices.BinarySearch(values[plugin], v)
		b.WriteString(o[last:m[4]])
		fmt.Fprintf(&b, "%s-T%d", plugin, k+1)
		last = m[5]
	}
	b.WriteString(o[last:])
	return b.String()
}

// configTree is a side's DynCfg tree for a caller (header lines), its clock values left for dcClock.
func (x *dcSide) configTree(t *testing.T, label string, headers ...string) string {
	t.Helper()
	return x.do(t, label, fnHTTPGet("/api/v1/config?action=tree", "", headers...))
}

// TestPluginsRealSet (check `plugins.real-set`, M8 commit 10, D181): the installed plugins under both agents at once.
func TestPluginsRealSet(t *testing.T) {
	if _, err := os.Stat("/run/systemd/system"); err != nil {
		t.Skip("no systemd on this host: the real set's long-runners differ (plan §9)")
	}
	cases := map[string]realCase{
		"set":            realSetCase(),
		"listener":       realListenerCase(),
		"functions":      realFunctionsCase(),
		"apps":           realAppsCase(),
		"journal-dyncfg": realJournalDynCfgCase(),
		"scripts-dyncfg": realScriptsDynCfgCase(),
	}
	if os.Getenv("PARITY_LONG") == "1" {
		cases["soak"] = realSoakCase()
	}
	runRealCases(t, cases)
}

// realSetCase is `set`: every installed plugin as installed. Compared: the plugin threads after the settle (the
// long-runners' 15-byte tags), each long-runner's context, the configuration dump, the lists of Functions, the DynCfg
// tree, the charts but apps.plugin's; after the stop, every plugin thread's records and the plugins' own.
func realSetCase() realCase {
	return realCase{
		running: []string{"apps", "network-viewer", "nfacct", "otel", "scripts.d", "systemd-journal", "systemd-units"},
		play: func(t *testing.T, x *dcSide) []string {
			var out []string
			for _, h := range []struct{ label, header string }{{"tree anonymous", ""}, {"tree admin", dcAdmin}} {
				var headers []string
				if h.header != "" {
					headers = append(headers, h.header)
				}
				out = append(out, realRankPerPlugin(x.configTree(t, h.label, headers...)))
			}
			return out
		},
		compare: func(t *testing.T, p *Pair) {
			var ctx [2]map[string][]string
			for i, side := range p.Each() {
				ctx[i] = realContexts(t, side.Daemon)
			}
			if len(ctx[0]) == 0 {
				t.Error("oracle: no plugin process found")
			}
			for _, file := range slices.Sorted(maps.Keys(ctx[0])) {
				diffLines(t, "the context of "+file, ctx[0][file], ctx[1][file])
			}
			if !slices.Equal(slices.Sorted(maps.Keys(ctx[0])), slices.Sorted(maps.Keys(ctx[1]))) {
				t.Errorf("running plugins: oracle %v, candidate %v", slices.Sorted(maps.Keys(ctx[0])), slices.Sorted(maps.Keys(ctx[1])))
			}
			t.Logf("compared the contexts of %v", slices.Sorted(maps.Keys(ctx[0])))
			compareNetdataConfOn(t, p)
			for _, side := range p.Each() {
				b, err := rawExchange(side.Daemon.Addr, []byte("GET /netdata.conf HTTP/1.1\r\n\r\n"), 5*time.Second)
				if err != nil || !bytes.Contains(b, []byte("\t# slabinfo = no\n")) {
					t.Errorf("%s: the dump has no `# slabinfo = no` (plugins_d.c:305): %v", side.Role, err)
				}
			}
			realCompareFunctions(t, p)
			realCompareCharts(t, p, "charts (apps.plugin's left out)", realNotApps)
			realCompareContexts(t, p, realNotAppsContext)
		},
		// every installed plugin but slabinfo (default no) started on the oracle: each has a thread's records
		after: func(t *testing.T, p *Pair) {
			classes := realOutcomes(t, p.Oracle, nil)
			for _, name := range realInstalled(t) {
				_, ok := classes["daemon.log thread="+realThreadTag(name)]
				if ok != (name != "slabinfo") {
					t.Errorf("oracle: plugin %s has records %v, want %v", name, ok, name != "slabinfo")
				}
			}
		},
		// (a long-runner's "exited abnormally" at the stop is the stop walk's race, realStopPairRe: no guard)
		want: []string{"exited with error code", "plugin called DISABLE", "does not generate useful output"},
	}
}

// realListenerCase is `listener`: with NETDATA_LISTENER_PORT set (as in container images) freeipmi defaults to no
// (plugins_d.c:309-310), so neither side starts it; the dump shows the key.
func realListenerCase() realCase {
	return realCase{
		env: []string{"NETDATA_LISTENER_PORT=19999"},
		compare: func(t *testing.T, p *Pair) {
			compareNetdataConfOn(t, p)
			for _, side := range p.Each() {
				b, err := rawExchange(side.Daemon.Addr, []byte("GET /netdata.conf HTTP/1.1\r\n\r\n"), 5*time.Second)
				if err != nil || !bytes.Contains(b, []byte("\t# freeipmi = no\n")) {
					t.Errorf("%s: the dump has no `# freeipmi = no`: %v", side.Role, err)
				}
			}
		},
		wantNot: []string{"PD[freeipmi]"},
	}
}

// realFunctions are the real plugins' Functions on this host (plan §3.2), each with the seconds its `info` answer
// expires in (the head's Expires, C vs C b1) and the seconds its `info` body's own expiry is set to (-1: none):
// processes now + 1 (apps_functions.c:901), network-viewer's now + 5 (network-viewer-topology.h:66). Each is called
// anonymously, as the member and as the admin (fnWriteTokens' tokens), its `info` compared whole and one data call by
// its shape.
var realFunctions = []struct {
	name       string
	head, body int
}{{"systemd-journal", 1, -1}, {"systemd-list-units", 3600, -1}, {"otel-logs", 2, -1}, {"otel-traces", 2, -1},
	{"processes", 1, 1}, {"topology:network-connections", 5, 5}, {"network-connections", 5, 5}, {"dns-queries", 0, -1}}

// realFnTx is a transaction the functions case sends.
func realFnTx(n int) string { return fnTx(0xa000 + n) }

// realFnTarget is the v1 call of a Function's command.
func realFnTarget(cmd string) string {
	return "/api/v1/function?function=" + strings.ReplaceAll(cmd, " ", "%20")
}

var (
	// realExpiresRe is an answer's expiry (`"expires":<seconds>`)
	realExpiresRe = regexp.MustCompile(`"expires":(\d{10})\b`)
	// realJournalReqRe are a journal `info` answer's window: now-3600 and now, as the plugin resolved them
	// (logs_query_status.h:663-696, :857-858; probe p1)
	realJournalReqRe = regexp.MustCompile(`"(after|before)":\d{10}`)
	// realJournalSourcesRe is the journal registry's version: the plugin's start in µs plus its file count
	// (systemd-journal-files.c:38-46, :900; probe p1)
	realJournalSourcesRe = regexp.MustCompile(`"versions":\{"sources":\d+\}`)
	// realJournalPillRe are a journal source's size and coverage: they follow journald's writes
	// (systemd-journal-files.c:497-598)
	realJournalPillRe = regexp.MustCompile(`"pill":"[^"]*","info":"[^"]*"`)
	// realJournalTimingRe are a journal file's scan timings in an answer (`_journal_files[]`,
	// systemd-journal-execute.h:615-649; C vs C, b1)
	realJournalTimingRe = regexp.MustCompile(`"(duration_ut|rows_per_second|bytes_per_second|duration_matches_ut)":[0-9.e+-]+`)
	// realJournalFileTimeRe are a journal file's last write and last message as the watcher last saw them
	// (`_journal_files[]`, systemd-journal-execute.h:615-649, systemd-journal-watcher.c:446-483): journald's writes
	realJournalFileTimeRe = regexp.MustCompile(`"(_last_modified_ut|_msg_last_ut)":\d+`)
)

// realExpires renders each expiry of body as its distance from date: the plugin writes now plus k seconds and the
// head's Date is taken after it, so a second boundary on the way makes the distance k-1: `NOW+k` for k or k-1, any
// other distance (or any, when k < 0) as it is.
func realExpires(body []byte, date time.Time, k int64) []byte {
	if date.IsZero() {
		return body
	}
	return realExpiresRe.ReplaceAllFunc(body, func(m []byte) []byte {
		e, err := strconv.ParseInt(string(realExpiresRe.FindSubmatch(m)[1]), 10, 64)
		if err != nil {
			return m
		}
		if d := e - date.Unix(); k < 0 || (d != k && d != k-1) {
			return []byte(fmt.Sprintf(`"expires":NOW%+d`, d))
		}
		return []byte(fmt.Sprintf(`"expires":NOW+%d`, k))
	})
}

// realHeadMask is fnHTTPMask with the head's length masked and its Expires as `Date+<head>` when it is that many
// seconds after Date or one less (the plugin takes its clock before the head's Date is taken), the body rendered by
// body first.
func realHeadMask(resp []byte, head int, body func(b []byte, date time.Time) []byte) string {
	h, b, ok := bytes.Cut(resp, []byte("\r\n\r\n"))
	if !ok {
		return fnHTTPMask(resp)
	}
	date := fnHTTPDate(resp)
	b = body(b, date)
	lines := strings.Split(string(contentLengthRe.ReplaceAll(h, []byte("Content-Length: N"))), "\r\n")
	for i, l := range lines {
		if v, ok := strings.CutPrefix(l, "Expires: "); ok && !date.IsZero() {
			if e, err := time.Parse(http.TimeFormat, v); err == nil {
				if d := int(e.Sub(date) / time.Second); d == head || d == head-1 {
					lines[i] = "Expires: " + date.Add(time.Duration(head)*time.Second).Format(http.TimeFormat)
				}
			}
		}
	}
	return fnHTTPMask(slices.Concat([]byte(strings.Join(lines, "\r\n")), []byte("\r\n\r\n"), b))
}

// realInfoMask renders an `info` answer (realHeadMask): the body's expiry (realExpires with `body` seconds) and, in
// the journal's, its now-relative window, its registry's version and its sources' sizes and coverage.
func realInfoMask(head, body int) func(resp []byte) string {
	return func(resp []byte) string {
		return realHeadMask(resp, head, func(b []byte, date time.Time) []byte {
			b = realExpires(b, date, int64(body))
			b = realJournalReqRe.ReplaceAll(b, []byte(`"$1":NOW`))
			b = realJournalSourcesRe.ReplaceAll(b, []byte(`"versions":{"sources":V}`))
			return realJournalPillRe.ReplaceAll(b, []byte(`"pill":P,"info":I`))
		})
	}
}

// realWindowMask renders the journal's fixed-window data answer (realHeadMask, no-cache): its expiry, now + 0
// (systemd-journal-execute.h:791-792), its registry's version, and per journal file its scan timings and its last
// write and message; the window itself (`_request.after/before`, the histogram's view) is the request's, compared.
func realWindowMask(resp []byte) string {
	return realHeadMask(resp, 0, func(b []byte, date time.Time) []byte {
		b = realExpires(b, date, 0)
		b = realJournalSourcesRe.ReplaceAll(b, []byte(`"versions":{"sources":V}`))
		b = realJournalTimingRe.ReplaceAll(b, []byte(`"$1":V`))
		return realJournalFileTimeRe.ReplaceAll(b, []byte(`"$1":V`))
	})
}

// realShapeStatic are the top-level members a data answer's shape keeps by value: what the plugin says of the
// Function, not of the box.
var realShapeStatic = []string{"status", "type", "v", "update_every", "has_history", "help", "default_sort_column",
	"show_ids", "error_message"}

// realShape renders a data answer by its shape (E5): the status line, the top-level members in order with their kinds
// (realShapeStatic's by value), a table's columns with their types, its rows' count masked (their lengths kept as a
// set), an object `data`'s members with their kinds.
func realShape(resp []byte) string {
	status := fnStatusLine(resp)
	v, err := ParseJSON(httpBody(resp))
	if err != nil {
		return status + " body " + strconv.Quote(truncateBytes(httpBody(resp)))
	}
	kind := func(v Value) string {
		return [...]string{"null", "bool", "number", "string", "array", "object", "masked"}[min(int(v.Kind), 6)]
	}
	out := []string{status}
	for _, m := range v.Members {
		line := "member " + m.Key + " " + kind(m.Value)
		if slices.Contains(realShapeStatic, m.Key) {
			line += " " + m.Value.String()
		}
		out = append(out, line)
		switch {
		case m.Key == "columns" && m.Value.Kind == KindObject:
			for _, c := range m.Value.Members {
				typ, _, _, _ := fnMember(c.Value, "type")
				out = append(out, "column "+c.Key+" "+typ.String())
			}
		case m.Key == "data" && m.Value.Kind == KindArray:
			var cells []string
			for _, row := range m.Value.Items {
				cells = append(cells, fmt.Sprintf("%s/%d", kind(row), len(row.Items)))
			}
			slices.Sort(cells)
			out = append(out, "rows N, cells "+strings.Join(slices.Compact(cells), " "))
		case m.Key == "data" && m.Value.Kind == KindObject:
			for _, d := range m.Value.Members {
				out = append(out, "data "+d.Key+" "+kind(d.Value))
			}
		}
	}
	return strings.Join(out, "; ")
}

// realFunctionsCase is `functions`: every real Function called by the anonymous caller (C's SSO 412), the member
// (the space 403), and the admin: its `info` compared whole (realInfoMask) and one data call by its shape
// (realShape); the journal's over a fixed past window. Compared too: the access records (the data calls' sizes follow
// the box) and the web workers' call records.
func realFunctionsCase() realCase {
	var window string
	// each Function's calls are n = 4k+1 (anonymous) to 4k+4 (data): the data calls' sizes follow the box (rows), the
	// journal's `info` (n = 3) its files' sizes
	sized := map[string]bool{realFnTx(3): true}
	for k := range realFunctions {
		sized[realFnTx(4*k+4)] = true
	}
	return realCase{
		running: []string{"apps", "network-viewer", "otel", "systemd-journal", "systemd-units"},
		play: func(t *testing.T, x *dcSide) []string {
			var out []string
			n := 0
			call := func(label, cmd, header string, mask func([]byte) string) {
				n++
				var headers []string
				if header != "" {
					headers = append(headers, header)
				}
				b, err := rawExchange(x.d.Addr, fnHTTPGet(realFnTarget(cmd), realFnTx(n), headers...), 60*time.Second)
				if err != nil {
					t.Errorf("%s: %s: %v", x.role, label, err)
					out = append(out, label+": "+err.Error())
					return
				}
				out = append(out, label+": "+strconv.Quote(mask(b)))
			}
			for _, f := range realFunctions {
				fn := f.name
				call(fn+" anonymous", fn, "", fnHTTPMask)
				call(fn+" member", fn, dcMember, fnHTTPMask)
				call(fn+" info", fn+" info", dcAdmin, realInfoMask(f.head, f.body))
				if fn == "systemd-journal" {
					call(fn+" window", fn+" "+window, dcAdmin, realWindowMask)
				} else {
					call(fn+" data", fn, dcAdmin, realShape)
				}
			}
			return out
		},
		compare: func(t *testing.T, p *Pair) {
			var calls [2][]string
			for i, side := range p.Each() {
				calls[i] = fnHTTPCallRecords(t, side.Daemon)
			}
			diffLines(t, "web workers' call records", calls[0], calls[1])
		},
		prepare: func(t *testing.T, runDir string) {
			if window == "" {
				// a fixed past window, the same on both sides: ten minutes ending an hour ago
				before := time.Now().Add(-time.Hour).Truncate(time.Minute).Unix()
				window = fmt.Sprintf("after:%d before:%d last:50 direction:backward", before-600, before)
			}
		},
		records: func(l string) string {
			for tx := range sized {
				if strings.Contains(l, " transaction=sent:"+tx+" ") {
					return responseBytesRe.ReplaceAllString(l, "${1}N")
				}
			}
			return l
		},
		want: []string{fnQ(fnBuiltinsSSO), fnQ(fnBuiltinsSpace), `\"type\":\"table\"`},
	}
}

// realPluginPID is the pid of a daemon's running plugin by file (a child of its spawn server; 0 when none).
func realPluginPID(d *daemon.Daemon, file string) int {
	server := spawnServerOf(d.PID())
	entries, _ := os.ReadDir("/proc")
	for _, e := range entries {
		pid, err := strconv.Atoi(e.Name())
		if err != nil {
			continue
		}
		if ppid, _, _ := procStat(pid); ppid != server || server == 0 {
			continue
		}
		cmdline, _ := os.ReadFile(fmt.Sprintf("/proc/%d/cmdline", pid))
		if argv0, _, _ := strings.Cut(string(cmdline), "\x00"); argv0 == filepath.Join(realPluginsDir(), file) {
			return pid
		}
	}
	return 0
}

// realPidParamRe is the pid of a `processes pid:<pid>` call.
var realPidParamRe = regexp.MustCompile(`pid:\d+`)

// realProcessCategory is the Category apps.plugin gives a pid in a side's `processes pid:<pid>` view
// (apps_functions.c:1114: the pid's row and its children's), "" when the view has no row for it.
func realProcessCategory(t *testing.T, x *dcSide, pid, n int) string {
	t.Helper()
	req := fnHTTPGet(realFnTarget(fmt.Sprintf("processes pid:%d", pid)), realFnTx(n), dcAdmin)
	b, err := rawExchange(x.d.Addr, req, 30*time.Second)
	if err != nil {
		t.Errorf("%s: processes pid:%d: %v", x.role, pid, err)
		return ""
	}
	v, err := ParseJSON(httpBody(b))
	if err != nil {
		t.Errorf("%s: processes pid:%d: %v: %s", x.role, pid, err, truncateBytes(b))
		return ""
	}
	columns, _, _, _ := fnMember(v, "columns")
	index := func(id string) int {
		c, _, _, _ := fnMember(columns, id)
		i, _, _, _ := fnMember(c, "index")
		k, _ := strconv.Atoi(i.Text)
		return k
	}
	data, _, _, _ := fnMember(v, "data")
	for _, row := range data.Items {
		if p, c := index("PID"), index("Category"); max(p, c) < len(row.Items) && row.Items[p].Text == strconv.Itoa(pid) {
			return row.Items[c].Text
		}
	}
	return ""
}

// realLatest are a chart's dimensions' largest values over its last 3 s (`/api/v1/data`, one point, grouped by max),
// nil when the chart has none.
func realLatest(d *daemon.Daemon, chart string) (map[string]float64, error) {
	b, err := rawExchange(d.Addr, fnHTTPGet("/api/v1/data?chart="+chart+
		"&after=-3&before=0&points=1&group=max&format=json&options=unaligned&harness=wait", ""), 10*time.Second)
	if err != nil {
		return nil, err
	}
	v, err := ParseJSON(httpBody(b))
	if err != nil {
		return nil, fmt.Errorf("%s: %v: %s", chart, err, truncateBytes(b))
	}
	labels, _, _, _ := fnMember(v, "labels")
	data, _, _, _ := fnMember(v, "data")
	if len(data.Items) == 0 {
		return nil, nil
	}
	out := map[string]float64{}
	for i, l := range labels.Items {
		if i > 0 && i < len(data.Items[0].Items) {
			out[l.Text], _ = strconv.ParseFloat(data.Items[0].Items[i].Text, 64)
		}
	}
	return out, nil
}

// realTargetLabels are the label keys apps.plugin names a target by, per chart type (apps_output.c:23-26).
var realTargetLabels = map[string]string{"app": "app_group", "user": "user", "usergroup": "user_group"}

// realTargetOf is an apps.plugin chart's target, `<type>/<label value>` ("" for a chart of no target).
func realTargetOf(chart Value) string {
	typ, _, _, _ := fnMember(chart, "type")
	labels, _, _, _ := fnMember(chart, "chart_labels")
	if key, ok := realTargetLabels[typ.Text]; ok {
		if v, _, _, ok := fnMember(labels, key); ok {
			return typ.Text + "/" + v.Text
		}
	}
	return ""
}

// realLiveTargets are the apps.plugin targets live on both sides now (E6): those whose `<type>.<name>_processes`
// latest values are above 0 on each (realLatest), by realTargetOf; all but one of the oracle's must be.
func realLiveTargets(t *testing.T, p *Pair) map[string]bool {
	t.Helper()
	var live [2]map[string]bool
	for i, side := range p.Each() {
		charts := realCharts(t, side.Daemon, func(id string, chart Value) bool {
			plugin, _, _, _ := fnMember(chart, "plugin")
			return plugin.Text == "apps.plugin" && strings.HasSuffix(id, "_processes")
		})
		live[i] = map[string]bool{}
		for _, group := range charts.Members {
			for _, c := range group.Value.Members {
				// a chart that cannot be read counts as not live (k80-r3: the Rust agent's `/api/v1/data` answered 404
				// for a transient target's chart its `/api/v1/charts` had just listed); the coverage guard below holds
				latest, err := realLatest(side.Daemon, c.Key)
				if err != nil {
					t.Logf("%s: %s counts as not live: %v", side.Role, c.Key, err)
				}
				if latest["processes"] > 0 {
					live[i][realTargetOf(c.Value)] = true
				}
			}
		}
	}
	both := map[string]bool{}
	for k := range live[0] {
		if live[1][k] {
			both[k] = true
		}
	}
	t.Logf("live targets: oracle %d, candidate %d, both %d: %v", len(live[0]), len(live[1]), len(both),
		slices.Sorted(maps.Keys(both)))
	// coverage: at most one of the oracle's live targets may be missing on the candidate (a process starting or ending
	// between the two reads); none was in the runs so far (34 of 34 every time)
	if len(both) < len(live[0])-1 {
		t.Errorf("only %d of the oracle's %d live targets are live on the candidate too", len(both), len(live[0]))
	}
	return both
}

// realLookupTotals are a side's network-viewer APPS_LOOKUP client request counts over the last 30 s (the chart's
// incremental rates summed): sent, responded and failed as zero or not.
func realLookupTotals(t *testing.T, x *dcSide) string {
	t.Helper()
	const chart = "netdata.collector_ipc_apps_lookup_client_requests"
	b, err := rawExchange(x.d.Addr, fnHTTPGet("/api/v1/data?chart="+chart+
		"&after=-30&before=0&points=1&group=sum&format=json&options=unaligned&harness=wait", ""), 10*time.Second)
	if err != nil {
		t.Errorf("%s: %s: %v", x.role, chart, err)
		return err.Error()
	}
	v, err := ParseJSON(httpBody(b))
	if err != nil {
		return "no data: " + truncateBytes(b)
	}
	labels, _, _, _ := fnMember(v, "labels")
	data, _, _, _ := fnMember(v, "data")
	if len(data.Items) == 0 {
		return "no rows"
	}
	var out []string
	for i, l := range labels.Items {
		if i == 0 || i >= len(data.Items[0].Items) {
			continue
		}
		f, _ := strconv.ParseFloat(data.Items[0].Items[i].Text, 64)
		out = append(out, fmt.Sprintf("%s>0:%v", l.Text, f > 0))
	}
	return strings.Join(out, " ")
}

// realAppsCase is `apps`: apps.plugin and network-viewer alone. Compared: `processes info`; the manager rule (each
// side's apps.plugin started by `spawn-plugins` gets its own category in every view, apps_targets.c:118-122: in each
// side's `processes pid:<p>` view the oracle's and the candidate's apps.plugin carry the same one); the APPS_LOOKUP
// pairing over each agent's run directory (after one `network-connections` call, network-viewer's lookups answered and
// none failed); apps.plugin's contexts; the definitions and labels of the charts of the targets live on both sides
// (realLiveTargets; values never).
func realAppsCase() realCase {
	var pids [2]int
	return realCase{
		only: []string{"apps", "network-viewer"},
		play: func(t *testing.T, x *dcSide) []string {
			var out []string
			b, err := rawExchange(x.d.Addr, fnHTTPGet(realFnTarget("processes info"), realFnTx(1), dcAdmin), 30*time.Second)
			if err != nil {
				t.Errorf("%s: processes info: %v", x.role, err)
			}
			out = append(out, "processes info: "+strconv.Quote(realInfoMask(1, 1)(b)))
			own, other := pids[0], pids[1]
			if x.role == Candidate {
				own, other = other, own
			}
			out = append(out, "own apps.plugin category "+realProcessCategory(t, x, own, 2),
				"other apps.plugin category "+realProcessCategory(t, x, other, 3))
			b, err = rawExchange(x.d.Addr, fnHTTPGet(realFnTarget("network-connections"), realFnTx(4), dcAdmin), 60*time.Second)
			if err != nil {
				t.Errorf("%s: network-connections: %v", x.role, err)
			}
			out = append(out, "network-connections: "+fnStatusLine(b))
			// the client sends its lookups after the answer (network-viewer.c:780-806); the chart every second
			time.Sleep(4 * time.Second)
			return append(out, "lookups "+realLookupTotals(t, x))
		},
		before: func(t *testing.T, p *Pair) {
			for i, side := range p.Each() {
				if pids[i] = realPluginPID(side.Daemon, "apps.plugin"); pids[i] == 0 {
					t.Fatalf("%s: no apps.plugin process", side.Role)
				}
			}
		},
		compare: func(t *testing.T, p *Pair) {
			var contexts [2][]string
			for i, side := range p.Each() {
				b, err := rawExchange(side.Daemon.Addr, fnHTTPGet("/api/v1/contexts", ""), 30*time.Second)
				if err != nil {
					t.Fatalf("%s: /api/v1/contexts: %v", side.Role, err)
				}
				v, err := ParseJSON(httpBody(b))
				if err != nil {
					t.Fatalf("%s: /api/v1/contexts: %v", side.Role, err)
				}
				ctx, _, _, _ := fnMember(v, "contexts")
				contexts[i] = slices.Sorted(slices.Values(memberKeys(ctx)))
			}
			diffLines(t, "contexts", contexts[0], contexts[1])
			live := realLiveTargets(t, p)
			for _, want := range []string{"app/apps.plugin", "app/netdata"} {
				if !live[want] {
					t.Errorf("target %s is not live on both sides: %v", want, slices.Sorted(maps.Keys(live)))
				}
			}
			realCompareCharts(t, p, "charts of the targets live on both sides", func(_ string, chart Value) bool {
				plugin, _, _, _ := fnMember(chart, "plugin")
				return plugin.Text == "apps.plugin" && (live[realTargetOf(chart)] || realTargetOf(chart) == "")
			})
			realCompareCharts(t, p, "network-viewer's charts", func(_ string, chart Value) bool {
				plugin, _, _, _ := fnMember(chart, "plugin")
				return plugin.Text == "network-viewer.plugin"
			})
		},
		want: []string{"own apps.plugin category apps.plugin", "other apps.plugin category apps.plugin",
			"requests_responded>0:true", "requests_failed>0:false"},
		// the pid views and the connections follow the box; each side asks for its own apps.plugin first
		records: func(l string) string {
			for _, n := range []int{2, 3, 4} {
				if strings.Contains(l, " transaction=sent:"+realFnTx(n)+" ") {
					l = realPidParamRe.ReplaceAllString(l, "pid:P")
					return responseBytesRe.ReplaceAllString(l, "${1}N")
				}
			}
			return l
		},
	}
}

// realJournalNode is systemd-journal.plugin's DynCfg node (systemd-journal-dyncfg.c:162-177).
const realJournalNode = "systemd-journal:monitored-directories"

// realJournalUpdate is an admin's update of the journal node with a payload, as the API takes it.
func realJournalUpdate(t *testing.T, x *dcSide, label string, n int, payload string) string {
	t.Helper()
	return x.send(t, label, "/api/v1/config?action=update&id="+realJournalNode, n, payload, dcAdmin)
}

// realJournalSaved waits until the node's tree entry shows a user's saved payload in effect again (`source_type`
// dyncfg: the restart's update echo answered, dyncfg-echo.c:32-34).
func realJournalSaved(t *testing.T, x *dcSide) bool {
	t.Helper()
	re := regexp.MustCompile(`"` + regexp.QuoteMeta(realJournalNode) +
		`":\{"type":"single","status":"running",.{0,400}?"source_type":"dyncfg"`)
	return x.until(t, "the journal node did not run its saved payload", "action=tree", re.Match)
}

// realJournalDynCfgCase is `journal-dyncfg`: systemd-journal.plugin alone, its DynCfg node driven by the admin (view
// 0x23, edit 0x47 by default, dyncfg.c:395-399): the tree, get, the stock schema (served from the oracle's
// `conf.d/schema.d`, never forwarded), updates the plugin refuses (`/etc`, bad JSON) and takes (two lists, one naming a
// missing directory), get after each, the saved file; then both agents restart and the saved payload is echoed to the
// plugin when it registers (dyncfg.c:286-298): the tree, get and the file again. The DynCfg records are compared too.
func realJournalDynCfgCase() realCase {
	ready := map[string]string{realJournalNode: "running"}
	return realCase{
		only: []string{"systemd-journal"},
		pair: func(t *testing.T, p *Pair, xs [2]*dcSide) [2][]string {
			return dcBoth(xs, func(_ int, x *dcSide) []string {
				var out []string
				if !x.waitTree(t, ready) {
					return out
				}
				out = append(out,
					x.get(t, "tree", "action=tree", 1, dcAdmin),
					x.get(t, "tree anonymous", "action=tree", 2),
					x.get(t, "get", "action=get&id="+realJournalNode, 3, dcAdmin),
					x.get(t, "get anonymous", "action=get&id="+realJournalNode, 4),
					x.get(t, "schema", "action=schema&id="+realJournalNode, 5, dcAdmin),
					realJournalUpdate(t, x, "update /etc", 6, `{"journalDirectories":["/etc"]}`),
					realJournalUpdate(t, x, "update bad json", 7, `{"journalDirectories":`),
					realJournalUpdate(t, x, "update relative", 8, `{"journalDirectories":["var/log/journal"]}`),
					realJournalUpdate(t, x, "update empty", 9, `{"journalDirectories":[]}`),
					x.get(t, "get after the refusals", "action=get&id="+realJournalNode, 10, dcAdmin),
					realJournalUpdate(t, x, "update one", 11, `{"journalDirectories":["/var/log/journal"]}`),
					x.get(t, "get after one", "action=get&id="+realJournalNode, 12, dcAdmin),
					realJournalUpdate(t, x, "update missing", 13,
						`{"journalDirectories":["/var/log/journal","/run/log/journal","/nonexistent-parity-journal"]}`),
					x.get(t, "get after missing", "action=get&id="+realJournalNode, 14, dcAdmin),
					x.get(t, "tree after the updates", "action=tree", 15, dcAdmin),
				)
				out = append(out, x.files(t, "files before the restart")...)
				if !realRestart(t, x) || !x.waitTree(t, ready) || !realJournalSaved(t, x) {
					return out
				}
				out = append(out,
					x.get(t, "tree after the restart", "action=tree", 16, dcAdmin),
					x.get(t, "get after the restart", "action=get&id="+realJournalNode, 17, dcAdmin))
				return append(out, x.files(t, "files after the restart")...)
			})
		},
		want: []string{`directory contains /etc`, `cannot parse json payload`,
			`only directories starting with / are accepted`, `no directories in the payload`,
			`added, but some directories are not found in the filesystem`, `\"source_type\":\"dyncfg\"`,
			`DYNCFG`},
	}
}

// The scripts.d nodes (scripts.d plugin composition/run.go:394-409: the vnode template, the four secret store
// templates in sorted order, the nagios template, then the file jobs).
const (
	realNagios     = "scripts.d:collector:nagios"
	realVnodeT     = "scripts.d:vnode"
	realVnodeGUID  = "5a1e0000-0000-4000-8000-0000000000e1"
	realVnodeName  = "parity-svnode"
	realNagiosConf = "jobs:\n  - name: file1\n    plugin: /usr/bin/true\n    update_every: 1\n    check_interval: 1\n"
)

// realNagiosJob is a nagios job's payload running /usr/bin/true (root-owned, as the job's path check requires,
// go.d pkg/pathvalidate/validate_unix.go:27-96) every second, bound to a vnode when set.
func realNagiosJob(vnode string) string {
	if vnode != "" {
		return `{"plugin":"/usr/bin/true","update_every":1,"check_interval":1,"vnode":"` + vnode + `"}`
	}
	return `{"plugin":"/usr/bin/true","update_every":1,"check_interval":1}`
}

// realJobState are a side's latest values of a nagios job's state charts (realLatest), `<chart> <dim>=<value> …`:
// every tick of /usr/bin/true is ok (scripts.d nagios/state.go:24-32).
func realJobState(t *testing.T, x *dcSide, job string) []string {
	t.Helper()
	var out []string
	for _, id := range []string{"nagios_" + job + ".job_execution_state_" + job,
		"nagios_" + job + ".perfdata.true.job.execution_state-nagios_job=" + job} {
		latest, err := realLatest(x.d, id)
		if err != nil {
			t.Errorf("%s: %v", x.role, err)
		}
		line := "state " + id
		for _, dim := range slices.Sorted(maps.Keys(latest)) {
			line += fmt.Sprintf(" %s=%v", dim, latest[dim])
		}
		out = append(out, line)
	}
	return out
}

// realModifiedUtRe is a DynCfg node's modification time in a tree.
var realModifiedUtRe = regexp.MustCompile(`"modified_ut\\?":(\d{16})`)

// realModifiedAfter renders the modification times after `at` as RESTARTED: after an agent restart the saved jobs come
// back by the agent's echoes, which the plugin activates at once, so their order is a race (C vs C, c2: api2's rank
// T26 against T27).
func realModifiedAfter(o string, at time.Time) string {
	return realModifiedUtRe.ReplaceAllStringFunc(o, func(m string) string {
		v, _ := strconv.ParseInt(realModifiedUtRe.FindStringSubmatch(m)[1], 10, 64)
		if v >= at.UnixMicro() {
			return strings.TrimSuffix(m, m[len(m)-16:]) + "RESTARTED"
		}
		return m
	})
}

// realDiscovererRe is scripts.d's file job source's reader.
var realDiscovererRe = regexp.MustCompile(`discoverer=file_(reader|watcher)`)

// realScriptsChartLines are a side's scripts.d charts but the chart engines' (realScriptsCharts), each chart's
// definition on a line, sorted: the plugin activates jobs added together at once, so their charts come in any order
// (C vs C, d1). It waits up to 30 s for the two jobs' charts, the same on two reads a second apart.
func realScriptsChartLines(t *testing.T, x *dcSide) []string {
	t.Helper()
	var v Value
	last := ""
	// the polls are tagged (how many there are is each side's timing), then one read is compared
	pollUntil(30*time.Second, func() bool {
		v = realChartsAt(t, x.d, "/api/v1/charts?harness=wait", realScriptsCharts)
		n := 0
		for _, g := range v.Members {
			for _, c := range g.Value.Members {
				if strings.Contains(c.Key, "job_execution_state_") {
					n++
				}
			}
		}
		settled := n >= 2 && v.String() == last
		last = v.String()
		time.Sleep(time.Second)
		return settled
	})
	v = realCharts(t, x.d, realScriptsCharts)
	var out []string
	for _, g := range v.Members {
		for _, c := range g.Value.Members {
			labels, _, _, _ := fnMember(c.Value, "chart_labels")
			labels.Members = slices.SortedFunc(slices.Values(labels.Members), func(a, b Member) int {
				return strings.Compare(a.Key, b.Key)
			})
			for i, m := range c.Value.Members {
				if m.Key == "chart_labels" {
					c.Value.Members[i].Value = labels
				}
			}
			out = append(out, "chart "+c.Key+" "+c.Value.String())
		}
	}
	slices.Sort(out)
	if len(out) == 0 {
		out = append(out, "no scripts.d chart")
	}
	return out
}

// realVnodeCharts are the scripts.d vnode's chart ids (`/host/<vnode>/api/v1/charts`, sorted), waited for up to 60 s
// until there are some, the same on two reads a second apart: vj1's batch (HOST_DEFINE, then its CHART lines) may be
// half parsed at a read.
func realVnodeCharts(x *dcSide) string {
	var charts []string
	pollUntil(60*time.Second, func() bool {
		ids, _, err := hostCharts(x.d, realVnodeName)
		slices.Sort(ids)
		settled := err == nil && len(ids) > 0 && slices.Equal(ids, charts)
		charts = ids
		time.Sleep(time.Second)
		return settled
	})
	return strings.Join(charts, " ")
}

// realScriptsCharts holds for scripts.d's charts but each job's chart engine's own (their count follows which
// collection phases took time, scripts.d jobruntime/job_v2_runtime.go:12-73).
func realScriptsCharts(id string, chart Value) bool {
	plugin, _, _, _ := fnMember(chart, "plugin")
	return plugin.Text == "scripts.d" && !strings.Contains(id, ".internal.chartengine.")
}

// realScriptsDynCfgCase is `scripts-dyncfg`: scripts.d.plugin alone (the go.d framework's DynCfg, D169 B7), with a
// file job in `<run>/etc/scripts.d/nagios.conf`. Compared, each step waiting for the statuses it leads to: the tree
// (six templates, the file job accepted then running on the agent's enable echo), a job's get and schema (the
// plugin's), an API job's add, get, test, disable, enable, restart and remove; a vnode added through `scripts.d:vnode`
// and a job bound to it (HOST_DEFINE from a real plugin: the vnode's charts); the file job's state values; the saved
// files; then both agents restart and the saved jobs come back by the agent's echoes. Also: scripts.d's charts but its
// chart engines', and the DynCfg records.
func realScriptsDynCfgCase() realCase {
	job := func(name string) string { return realNagios + ":" + name }
	vnode := realVnodeT + ":pv1"
	return realCase{
		only: []string{"scripts.d"},
		prepare: func(t *testing.T, runDir string) {
			dcWriteFile(t, filepath.Join(runDir, "etc", "scripts.d", "nagios.conf"), []byte(realNagiosConf))
		},
		pair: func(t *testing.T, p *Pair, xs [2]*dcSide) [2][]string {
			return dcBoth(xs, func(_ int, x *dcSide) []string {
				var out []string
				get := func(label, query string, n int) { out = append(out, x.get(t, label, query, n, dcAdmin)) }
				send := func(label, query string, n int, body string) {
					out = append(out, x.send(t, label, "/api/v1/config?"+query, n, body, dcAdmin))
				}
				wait := func(want map[string]string) bool { return x.waitTree(t, want) }
				if !wait(map[string]string{realNagios: "accepted", realVnodeT: "accepted", job("file1"): "running"}) {
					return out
				}
				get("tree", "action=tree", 101)
				get("file job get", "action=get&id="+job("file1"), 102)
				get("file job schema", "action=schema&id="+job("file1"), 103)
				get("file job remove", "action=remove&id="+job("file1"), 104)
				send("add api1", "action=add&id="+realNagios+"&name=api1", 105, realNagiosJob(""))
				if !wait(map[string]string{job("api1"): "running"}) {
					return out
				}
				get("api1 get", "action=get&id="+job("api1"), 106)
				get("api1 test", "action=test&id="+job("api1"), 107)
				send("api1 test with a payload", "action=test&id="+job("api1"), 117, realNagiosJob(""))
				get("api1 disable", "action=disable&id="+job("api1"), 108)
				if !wait(map[string]string{job("api1"): "disabled"}) {
					return out
				}
				get("api1 enable", "action=enable&id="+job("api1"), 109)
				if !wait(map[string]string{job("api1"): "running"}) {
					return out
				}
				get("api1 restart", "action=restart&id="+job("api1"), 110)
				if !wait(map[string]string{job("api1"): "running"}) {
					return out
				}
				get("api1 remove", "action=remove&id="+job("api1"), 111)
				if !x.until(t, "api1 stayed in the tree", "action=tree", func(b []byte) bool {
					return !bytes.Contains(b, []byte(`"`+job("api1")+`"`))
				}) {
					return out
				}
				send("add vnode pv1", "action=add&id="+realVnodeT+"&name=pv1", 112,
					`{"guid":"`+realVnodeGUID+`","hostname":"`+realVnodeName+`"}`)
				if !wait(map[string]string{vnode: "running"}) {
					return out
				}
				send("add vj1", "action=add&id="+realNagios+"&name=vj1", 113, realNagiosJob("pv1"))
				send("add api2", "action=add&id="+realNagios+"&name=api2", 114, realNagiosJob(""))
				if !wait(map[string]string{job("vj1"): "running", job("api2"): "running"}) {
					return out
				}
				out = append(out, "vnode charts "+realVnodeCharts(x))
				get("tree after the adds", "action=tree", 115)
				out = append(out, realJobState(t, x, "file1")...)
				out = append(out, realScriptsChartLines(t, x)...)
				out = append(out, x.files(t, "files before the restart")...)
				restarted := time.Now()
				if !realRestart(t, x) || !wait(map[string]string{job("file1"): "running", job("api2"): "running",
					job("vj1"): "running", vnode: "running"}) {
					return out
				}
				out = append(out, realModifiedAfter(x.get(t, "tree after the restart", "action=tree", 116, dcAdmin), restarted))
				// vj1 defines the vnode again at its first collection after the restart (HOST_DEFINE: "VNODE: Configuring
				// node stale after …", and at the run's end "Checking virtual status", "Reseting virtual host status"):
				// both sides wait for it, so whether it came before the stop is not timing (run 53 on k80: real1 and
				// real3 had it on the oracle only, real2 on the candidate only)
				out = append(out, "vnode charts after the restart "+realVnodeCharts(x))
				return append(out, x.files(t, "files after the restart")...)
			})
		},
		// scripts.d names a file job's source by the reader that delivered the file first: the one-shot reader or the
		// watcher (go.d agent/discovery/file read.go:217, watch.go:143-145; C vs C, e1: after the restart one side's
		// file1 came from each)
		mask: func(obs []string) []string {
			for i, o := range obs {
				obs[i] = realDiscovererRe.ReplaceAllString(o, "discoverer=file_*")
			}
			return obs
		},
		// the tree holds the file job's source, the file's path in each side's run directory
		records: func(l string) string {
			if strings.Contains(l, `request="/api/v1/config?action=tree"`) {
				return responseBytesRe.ReplaceAllString(l, "${1}N")
			}
			return l
		},
		want: []string{`\"status\":202`, `scripts.d:collector:nagios:file1`,
			`files before the restart: scripts.d%3Avnode%3Apv1.dyncfg`, "vnode charts nagios_vj1.",
			"vnode charts after the restart nagios_vj1.", "ok=1", "VNODE: Configuring node stale after",
			"Reseting virtual host status for " + realVnodeName},
	}
}

// realSoakCase is `soak` (PARITY_LONG): the stock set held 150 s, past the 120 s read timeout (pluginsd_parser.c:
// 1452-1454), which otel-plugin holds off with PLUGIN_KEEPALIVE every 60 s: no long-runner restarts (each keeps its
// pid on both sides) and the records compare as `set`'s.
func realSoakCase() realCase {
	c := realSetCase()
	c.compare = nil
	c.play = func(t *testing.T, x *dcSide) []string {
		var files []string
		for _, n := range c.running {
			file := n + ".plugin"
			if n == "otel" {
				file = "otel-plugin"
			}
			files = append(files, file)
		}
		pids := map[string]int{}
		for _, f := range files {
			pids[f] = realPluginPID(x.d, f)
		}
		time.Sleep(150 * time.Second)
		var out []string
		for _, f := range files {
			now := realPluginPID(x.d, f)
			out = append(out, fmt.Sprintf("%s running %v, the same process %v", f, now != 0, now == pids[f] && now != 0))
		}
		return out
	}
	c.want = append(c.want, "otel-plugin running true, the same process true")
	return c
}

// TestRealSplitRecord pins realSplitRecord and realOwnRecords' rejoining of a raw line a record split (C vs C, h2).
func TestRealSplitRecord(t *testing.T) {
	const record = `time=2026-10-03T11:10:58.507Z comm=netdata source=collector level=warning tid=1 thread=PD[ioping] msg="x"`
	cases := map[string]struct{ line, record, fragment string }{
		"a record":          {record, record, ""},
		"a raw line":        {"xc_interface_open: No such file or directory", "xc_interface_open: No such file or directory", ""},
		"a split raw line":  {"xencall: error: " + record, "time=T " + record[len("time=2026-10-03T11:10:58.507Z "):], "xencall: error: "},
		"time= in raw text": {"python.d: time=5", "python.d: time=5", ""},
	}
	for name, c := range cases {
		if r, f := realSplitRecord(c.line); r != c.record || f != c.fragment {
			t.Errorf("%s: got (%q, %q), want (%q, %q)", name, r, f, c.record, c.fragment)
		}
	}
	run := t.TempDir()
	dcWriteFile(t, filepath.Join(run, "log", "collector.log"), []byte("xencall: error: "+record+"\n"+
		"Could not obtain handle on privileged command interface: No such file or directory\n"+
		"xc_interface_open: No such file or directory\n"))
	got := realOwnRecords(t, &daemon.Daemon{Opts: daemon.Options{RunDir: run}}, nil)
	want := map[string][]string{"raw": {
		"xc_interface_open: No such file or directory",
		"xencall: error: Could not obtain handle on privileged command interface: No such file or directory",
	}}
	if fmt.Sprint(got) != fmt.Sprint(want) {
		t.Errorf("got %q, want %q", got, want)
	}
}

// TestRealRankPerPlugin pins realRankPerPlugin on a quoted tree: each plugin's node times ranked among its own.
func TestRealRankPerPlugin(t *testing.T) {
	tree := `{"tree":{"/a":{"scripts.d:t1":{"type":"template","created_ut":1790000000000005,"modified_ut":1790000000000005},` +
		`"scripts.d:t2":{"type":"template","created_ut":1790000000000009,"modified_ut":1790000000000009}},` +
		`"/b":{"systemd-journal:n":{"type":"single","created_ut":1790000000000007,"modified_ut":1790000000000008}}},` +
		`"agent":{"now":1790000000}}`
	got := realRankPerPlugin("tree: " + strconv.Quote(tree))
	want := "tree: " + strconv.Quote(`{"tree":{"/a":{"scripts.d:t1":{"type":"template","created_ut":scripts.d-T1,`+
		`"modified_ut":scripts.d-T1},"scripts.d:t2":{"type":"template","created_ut":scripts.d-T2,"modified_ut":scripts.d-T2}},`+
		`"/b":{"systemd-journal:n":{"type":"single","created_ut":systemd-journal-T1,"modified_ut":systemd-journal-T2}}},`+
		`"agent":{"now":1790000000}}`)
	if got != want {
		t.Errorf("got  %s\nwant %s", got, want)
	}
}

// TestRealParserState pins realParserState: a plugin thread's record with and without its parser's chart reads the
// same (the empty fields leave their spaces), its node masked; another thread's record is kept.
func TestRealParserState(t *testing.T) {
	const pre = `time=T comm=netdata source=daemon level=notice tid=N thread=PD[scripts.d] module=DYNCFG`
	cases := map[string]struct{ line, want string }{
		"with the chart": {pre + ` node=parity-svnode instance=a.b context=c.d src_transport=pluginsd msg="x"`,
			pre + ` node=N   src_transport=pluginsd msg="x"`},
		"without it": {pre + ` node=parity-parent   src_transport=pluginsd msg="x"`,
			pre + ` node=N   src_transport=pluginsd msg="x"`},
		"a web worker's": {`time=T thread=WEB[n] node=parity-parent instance=a.b context=c.d msg="x"`,
			`time=T thread=WEB[n] node=parity-parent instance=a.b context=c.d msg="x"`},
	}
	for name, c := range cases {
		if got := realParserState(c.line); got != c.want {
			t.Errorf("%s:\ngot  %q\nwant %q", name, got, c.want)
		}
	}
}

// TestRealStopPair pins realOutcomes' stop mask: a plugin thread's kill records are dropped only inside a stop's
// window, every other record and every record outside a stop kept.
func TestRealStopPair(t *testing.T) {
	at := func(s string) string {
		return "time=2026-10-03T12:00:" + s + "Z comm=netdata source=daemon level=info tid=1 thread=PD[x] "
	}
	abnormal := `msg="PLUGINSD: 'host:h', '/p/x.plugin' (pid 7) exited abnormally. Disabling it."`
	run := t.TempDir()
	dcWriteFile(t, filepath.Join(run, "log", "daemon.log"), []byte(at("01.000")+abnormal+"\n"+
		at("05.000")+`msg="PLUGINSD: plugin called DISABLE. Disabling it."`+"\n"+at("05.500")+abnormal+"\n"))
	dcWriteFile(t, filepath.Join(run, "log", "collector.log"), []byte(at("05.600")+
		`msg="SPAWN PARENT: giving up waiting for pid 7 after SIGKILL (request No 3) - reclaiming"`+"\n"))
	stop, _ := time.Parse(time.RFC3339Nano, "2026-10-03T12:00:04Z")
	got := realOutcomes(t, &daemon.Daemon{Opts: daemon.Options{RunDir: run}}, []realStop{{begin: stop}})
	want := map[string][]string{
		"daemon.log thread=PD[x]": {
			"time=T comm=netdata source=daemon level=info tid=N thread=PD[x] " + strings.Replace(abnormal, "pid 7", "pid P", 1),
			`time=T comm=netdata source=daemon level=info tid=N thread=PD[x] msg="PLUGINSD: plugin called DISABLE. Disabling it."`,
		},
		"collector.log thread=PD[x]": nil,
	}
	if fmt.Sprint(got) != fmt.Sprint(want) {
		t.Errorf("got  %q\nwant %q", got, want)
	}
}
