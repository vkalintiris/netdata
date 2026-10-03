// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"sort"
	"strconv"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/plugin"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// capture is a child's stream as a recording parent received it, normalized for comparison (brief
// `knowledge/brief-stream-sender.md` §6): the request's parameters and headers, the session's start (host labels
// as a set: C prints them in heap order), each chart's definition keyed by id (its times masked) and how many times
// it came, and each chart's data as the dimensions and flags of its blocks (values and times masked, the number of
// blocks left out; v1 blocks with their time reduced to zero or not); slot numbers masked, each chart's blocks
// checked against its definition's slot; chart labels as a set.
type capture struct {
	request []string
	start   []string
	charts  map[string][]string
	// defs counts each chart's definitions: a version that moves at every collection would redefine it each time; a
	// chart's entry in charts holds every definition, in order
	defs map[string]int
	data map[string][]string
	// v1zero counts each chart's v1 blocks sent with no time since the last update (the resync horizon)
	v1zero map[string]int
	// dimSlots are each chart's dimensions' slots in its last definition, which its SET2 lines must use
	dimSlots map[string]map[string]string
	// replays are each chart's replication answers: their lines' kinds and dimensions
	replays map[string][]string
	// relists are the session's function re-lists in order, each its `FUNCTION_DEL GLOBAL` and `FUNCTION GLOBAL`
	// lines (one commit, so they come together), joined
	relists []string
	other   []string
	// slots are each chart's slot in its definition, which its blocks must use (the numbers follow the charts'
	// creation order, which varies)
	slots map[string]string
}

var (
	slotRe      = regexp.MustCompile(`SLOT:\S+ `)
	localPortRe = regexp.MustCompile(`127\.0\.0\.1:\d+`)
	replayRe    = regexp.MustCompile(`^(RBEGIN|RSET|RDSTATE|RSSTATE|REND)(?: SLOT:\S+)?(?: ('[^']*'|"[^"]*"))?`)
	// C's REND writes two spaces after true
	replayVerdictRe = regexp.MustCompile(` (true  |false )`)
	chartLineRe     = regexp.MustCompile(`^CHART (?:SLOT:\S+ )?"([^"]*)"`)
	begin2Re        = regexp.MustCompile(`^BEGIN2 (?:SLOT:\S+ )?'([^']*)' `)
	set2Re          = regexp.MustCompile(`^SET2 (?:(SLOT:\S+) )?'([^']*)' \S+ \S+ (\S*)$`)
	dimensionRe     = regexp.MustCompile(`^DIMENSION (?:(SLOT:\S+) )?"([^"]*)"`)
	definitionEndRe = regexp.MustCompile(`^CHART_DEFINITION_END .*`)
	v1BeginRe       = regexp.MustCompile(`^BEGIN "([^"]*)" (\d+)$`)
	v1SetRe         = regexp.MustCompile(`^SET "([^"]*)" = \S+$`)
)

// parseCapture splits a plaintext stream into its parts.
func parseCapture(req stream.Request, data []byte) capture {
	c := capture{charts: map[string][]string{}, defs: map[string]int{}, data: map[string][]string{},
		slots: map[string]string{}, replays: map[string][]string{}, v1zero: map[string]int{},
		dimSlots: map[string]map[string]string{}}
	// data must follow their chart's definition in the session
	defined := func(chart, what string) {
		if c.defs[chart] == 0 {
			c.other = append(c.other, what+" of "+chart+" before its definition")
		}
	}
	params := make([]string, 0, len(req.Params))
	for k, v := range req.Params {
		if k == "ml_capable" {
			// no ML here (D101.5)
			v = []string{"M"}
		}
		params = append(params, k+"="+strings.Join(v, ","))
	}
	sort.Strings(params)
	c.request = append(params, req.Headers...)
	var chart string // the chart whose definition or block is being read
	var labels, clabels []string
	var block []string
	v1Time := "" // the open v1 block's time: 0, or N for any other
	inPath := false
	var relist []string
	inRelist := false
	endRelist := func() {
		if len(relist) > 0 {
			c.relists = append(c.relists, strings.Join(relist, "\n"))
		}
		relist, inRelist = nil, false
	}
	for _, line := range strings.Split(string(data), "\n") {
		if strings.HasPrefix(line, "FUNCTION GLOBAL ") || strings.HasPrefix(line, "FUNCTION_DEL GLOBAL ") {
			inRelist = true
			relist = append(relist, line)
			continue
		}
		if inRelist && line != "" {
			endRelist()
		}
		switch {
		case line == "":
		case inPath:
			if line == "JSON_PAYLOAD_END" {
				inPath = false
				c.start = append(c.start, "JSON STREAM_PATH <payload>")
			}
		case line == "JSON STREAM_PATH":
			inPath = true
		case strings.HasPrefix(line, "LABEL "):
			// each child's parent listens on its own port
			labels = append(labels, localPortRe.ReplaceAllString(line, "127.0.0.1:P"))
		case line == "OVERWRITE labels":
			sort.Strings(labels)
			c.start = append(append(c.start, labels...), line)
			labels = nil
		case strings.HasPrefix(line, "CLAIMED_ID "), strings.HasPrefix(line, "VARIABLE HOST "):
			c.start = append(c.start, line)
		case chartLineRe.MatchString(line):
			chart = chartLineRe.FindStringSubmatch(line)[1]
			c.slots[chart] = slotRe.FindString(line)
			if c.defs[chart] > 0 {
				c.charts[chart] = append(c.charts[chart], "--- defined again")
			}
			c.charts[chart] = append(c.charts[chart], slotRe.ReplaceAllString(line, "SLOT:N "))
			c.defs[chart]++
			c.dimSlots[chart] = map[string]string{}
			clabels = nil
		case strings.HasPrefix(line, "CLABEL "):
			clabels = append(clabels, line)
		case line == "CLABEL_COMMIT":
			// C prints a chart's labels in heap order
			sort.Strings(clabels)
			c.charts[chart] = append(append(c.charts[chart], clabels...), line)
			clabels = nil
		case strings.HasPrefix(line, "DIMENSION "), strings.HasPrefix(line, "VARIABLE CHART "):
			if m := dimensionRe.FindStringSubmatch(line); m != nil && c.dimSlots[chart] != nil {
				c.dimSlots[chart][m[2]] = m[1]
			}
			c.charts[chart] = append(c.charts[chart], slotRe.ReplaceAllString(line, "SLOT:N "))
		case definitionEndRe.MatchString(line):
			c.charts[chart] = append(c.charts[chart], "CHART_DEFINITION_END <times>")
		case begin2Re.MatchString(line):
			chart = begin2Re.FindStringSubmatch(line)[1]
			defined(chart, "BEGIN2")
			if slot := slotRe.FindString(line); slot != c.slots[chart] {
				c.other = append(c.other, "BEGIN2 of "+chart+" in "+slot+", its definition's "+c.slots[chart])
			}
			block = nil
		case set2Re.MatchString(line):
			m := set2Re.FindStringSubmatch(line)
			if want := c.dimSlots[chart][m[2]]; m[1] != want {
				c.other = append(c.other, "SET2 of "+chart+"/"+m[2]+" in "+m[1]+", its definition's "+want)
			}
			block = append(block, m[2]+" "+m[3])
		case replayRe.MatchString(line):
			m := replayRe.FindStringSubmatch(line)
			if m[1] == "RBEGIN" && m[2] != "''" {
				chart = strings.Trim(m[2], "'")
				c.replays[chart] = nil
				continue
			}
			// the line's kind and dimension; points, values and times masked, REND's verdict kept
			shape := m[1]
			if m[1] == "RSET" || m[1] == "RDSTATE" {
				shape += " " + m[2]
			}
			if m[1] == "REND" {
				shape += " " + replayVerdictRe.FindString(line)
			}
			if m[1] != "RBEGIN" && m[1] != "RSET" || !slices.Contains(c.replays[chart], shape) {
				c.replays[chart] = append(c.replays[chart], shape)
			}
		case line == "END2":
			shape := strings.Join(block, ",")
			if !slices.Contains(c.data[chart], shape) {
				c.data[chart] = append(c.data[chart], shape)
			}
		case v1BeginRe.MatchString(line):
			m := v1BeginRe.FindStringSubmatch(line)
			chart, block, v1Time = m[1], nil, "N"
			defined(chart, "BEGIN")
			if m[2] == "0" {
				v1Time = "0"
				c.v1zero[chart]++
			}
		case v1SetRe.MatchString(line):
			block = append(block, v1SetRe.FindStringSubmatch(line)[1])
		case line == "END":
			shape := "v1 " + v1Time + " " + strings.Join(block, ",")
			if !slices.Contains(c.data[chart], shape) {
				c.data[chart] = append(c.data[chart], shape)
			}
		default:
			c.other = append(c.other, line)
		}
	}
	endRelist()
	for id := range c.data {
		sort.Strings(c.data[id])
	}
	return c
}

// compareCaptures reports how two captures differ.
func compareCaptures(t *testing.T, stage string, a, b capture) {
	t.Helper()
	diff := func(what string, x, y []string) {
		if !slices.Equal(x, y) {
			t.Errorf("%s: %s differ:\noracle:\n%s\ncandidate:\n%s", stage, what, strings.Join(x, "\n"),
				strings.Join(y, "\n"))
		}
	}
	diff("requests", a.request, b.request)
	diff("session starts", a.start, b.start)
	keys := func(m map[string][]string) []string {
		var k []string
		for id := range m {
			k = append(k, id)
		}
		sort.Strings(k)
		return k
	}
	diff("charts", keys(a.charts), keys(b.charts))
	for _, id := range keys(a.charts) {
		if y, ok := b.charts[id]; ok {
			diff("chart "+id, a.charts[id], y)
			if a.defs[id] != b.defs[id] {
				t.Errorf("%s: chart %s defined %d times by the oracle, %d by the candidate", stage, id, a.defs[id],
					b.defs[id])
			}
		}
	}
	diff("charts with data", keys(a.data), keys(b.data))
	for _, id := range keys(a.charts) {
		if a.v1zero[id] != b.v1zero[id] {
			t.Errorf("%s: chart %s sent %d v1 blocks with no time by the oracle, %d by the candidate", stage, id,
				a.v1zero[id], b.v1zero[id])
		}
	}
	for _, id := range keys(a.data) {
		if y, ok := b.data[id]; ok {
			diff("data of "+id, a.data[id], y)
		}
	}
	diff("replicated charts", keys(a.replays), keys(b.replays))
	for _, id := range keys(a.replays) {
		if y, ok := b.replays[id]; ok {
			diff("replication of "+id, a.replays[id], y)
		}
	}
	diff("function re-lists", a.relists, b.relists)
	diff("other lines", a.other, b.other)
}

// senderChild boots a daemon streaming to `parent` as a child with its own identity, `extra` in its [stream]
// section, and `adjust` applied to its options.
func senderChild(t *testing.T, bin string, role Role, parent *stream.Parent, extra string,
	adjust ...func(*daemon.Options)) *daemon.Daemon {
	t.Helper()
	id := daemon.Identity{Hostname: "sender-child", StreamKey: "5a1e0000-0000-4000-8000-0000000000c1",
		MachineGUID: "5a1e0000-0000-4000-8000-0000000000cc"}
	o := daemon.Options{Binary: bin, RunDir: runDir(t, role), Identity: &id, DBMode: "alloc", StorageTiers: 1,
		NoStreamKey: true, StreamTo: &daemon.StreamTo{Destination: parent.Addr(), APIKey: parentIdentity.StreamKey,
			Extra: "    reconnect delay = 5\n" + extra}}
	for _, f := range adjust {
		f(&o)
	}
	d, err := daemon.Start(o)
	if err != nil {
		t.Fatalf("parity: start %s: %v", role, err)
	}
	t.Cleanup(func() { _ = d.Stop() })
	return d
}

// senderRecordRe are the records of the per-collection gate and of the connection's reset, in the order the
// collector thread writes them.
var senderRecordRe = regexp.MustCompile(`msg="(STREAM SND '[^']*': streaming is (not )?ready|STREAM REPLAY: sender replicating-charts counter)`)

// senderRecords are a child's gate and reset records, in order.
func senderRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if m := senderRecordRe.FindString(l); m != "" {
			out = append(out, threadOf(l)+" "+m)
		}
	}
	return out
}

// senderVariant is what a recording parent negotiates with a sender child, and what happens during the session.
type senderVariant struct {
	name string
	// refused are the capabilities the parent does not answer, besides compression
	refused uint32
	// extra is the child's [stream] section lines
	extra string
	// during runs while the first session streams
	during func(t *testing.T, d *daemon.Daemon, s *stream.Session)
	// sessions are the sessions captured and compared
	sessions int
	// long runs only with PARITY_LONG
	long bool
	// child adjusts the child's options
	child func(*daemon.Options)
	// paths, when set, is how many stream paths the first session's start must hold on each side
	paths int
	// startHas, when set, is a pattern a line of the oracle's first session's start must match
	startHas string
	// plugin, when set, is the fake plugin's scenario: installed in the child's run directory and enabled
	plugin *plugin.Scenario
	// relists, when set, are the oracle's function re-lists per session (capture.relists), checked exactly
	relists [][]string
	// hold, when set, runs before the parent answers each STREAM request after the first: the child's handshake
	// waits for it (C's `[stream] timeout` defaults to 300 s, stream-conf.c:52)
	hold func(t *testing.T, d *daemon.Daemon)
}

var senderVariants = []senderVariant{
	// each chart's replication answered (commit 6), then its live data
	{name: "replication"},
	{name: "norepl", refused: stream.CapReplication},
	{name: "hex", refused: stream.CapReplication | stream.CapSlots | stream.CapIEEE754 | stream.CapFloatBaseline},
	{name: "v1", refused: stream.CapReplication | stream.CapInterpolated,
		extra: "    initial clock resync iterations = 3\n"},
	// a definition after a reconnect waits for the resync horizon: v1 blocks with no time until then
	{name: "v1-reconnect", refused: stream.CapReplication | stream.CapInterpolated, sessions: 2,
		extra: "    initial clock resync iterations = 3\n",
		during: func(t *testing.T, _ *daemon.Daemon, s *stream.Session) {
			time.Sleep(8 * time.Second)
			_ = s.Close()
		}},
	{name: "nolabels",
		refused: stream.CapReplication | stream.CapCLabels | stream.CapHLabels | stream.CapClaim | stream.CapPaths},
	// `[global] is ephemeral node`, and the legacy `[health] is ephemeral` moved to it, set the `_is_ephemeral` host
	// label (M7 10f, D122.11; the harness's children send it "false")
	{name: "ephemeral", refused: stream.CapReplication, startHas: `^LABEL "_is_ephemeral" = \d+ "true"$`,
		child: func(o *daemon.Options) { o.GlobalExtra = "    is ephemeral node = yes\n" }},
	{name: "ephemeral-legacy", refused: stream.CapReplication, startHas: `^LABEL "_is_ephemeral" = \d+ "true"$`,
		child: func(o *daemon.Options) { o.HealthExtra = "    is ephemeral = yes\n" }},
	{name: "pattern", refused: stream.CapReplication,
		extra: "    send charts matching = !netdata.http_api_* !netdata.network_streaming *\n"},
	{name: "reconnect", refused: stream.CapReplication, sessions: 2,
		during: func(t *testing.T, _ *daemon.Daemon, s *stream.Session) {
			time.Sleep(8 * time.Second)
			_ = s.Close()
		}},
	{name: "reload-labels", refused: stream.CapReplication,
		during: func(t *testing.T, d *daemon.Daemon, _ *stream.Session) {
			time.Sleep(6 * time.Second)
			if r := runCLI(t, d, "reload-labels"); r.Exit != 0 {
				t.Errorf("reload-labels: exit %d: %s", r.Exit, r.Stderr)
			}
		}},
	// the claim goes up again after the command, as C's claim_reload_and_wait_online() sends it (D126.2)
	// (a short name: the role names the run directory, which holds the command pipe)
	{name: "reload-claim", refused: stream.CapReplication,
		during: func(t *testing.T, d *daemon.Daemon, _ *stream.Session) {
			time.Sleep(6 * time.Second)
			if r := runCLI(t, d, "reload-claiming-state"); r.Exit != 0 {
				t.Errorf("reload-claiming-state: exit %d: %s", r.Exit, r.Stderr)
			}
		}},
	// localhost's first retention time rises after the sender is ready, so its path goes up again (D120.4): an
	// alloc child keeping 60 points, and a grandchild's chart freed 10 s after it goes obsolete, which arms the full
	// retention pass 120 s later
	{name: "retention-change", long: true, refused: stream.CapReplication, paths: 2,
		child: func(o *daemon.Options) {
			o.NoStreamKey = false
			o.StreamMemoryMode = "alloc"
			o.DBExtra = "    retention = 60\n    cleanup obsolete charts after = 10s\n"
			o.StreamExtra = "\n[" + senderGrandchild.MachineGUID + "]\n    type = machine\n    proxy enabled = no\n"
		},
		during: retentionChange},
	// the function re-list (fn-relist, M8 commit 5, D147.14 R61-3): the fake plugin's methods as the parent sees
	// them, with the parent taking FUNCTION_DEL, refusing it, and refusing FUNCTIONS
	fnRelistVariant("fn-relist", 0, [][]string{fnRelistBursts(true), {fnRelistJoin(nil)}}),
	fnRelistVariant("fn-nodel", stream.CapFunctionDel, [][]string{fnRelistBursts(false), {fnRelistJoin(nil)}}),
	// without FUNCTIONS the second session has no re-list at all (no connect-time send, no change after it)
	fnRelistVariant("fn-nofn", stream.CapFunctions, [][]string{fnRelistBursts(true), nil}),
	// DynCfg nodes (M8 commit 8, plan §5.4): each registration and delete of a `config <id>` method re-lists at the next
	// collection (rrdhost_nrpc_changed sets the flag for any registry change, rrdhost.c:353-360), and the re-list holds
	// the built-ins and `config` (DynCfg methods are never listed, nrpc-catalog.c:50-53; their deletes never queue
	// FUNCTION_DEL, nrpc-registry.c:699-703); with DYNCFG refused, the same re-lists without the `config` line
	dcRelistVariant("dc-relist", 0, [][]string{slices.Repeat([]string{fnRelistJoin(nil)}, 5)}),
	dcRelistVariant("dc-nodyncfg", stream.CapDynCfg, [][]string{slices.Repeat([]string{strings.Join(fnBuiltinRelist(), "\n")}, 5)}),
}

// fnRelistLine is a method's re-list line as C renders it (nrpc-catalog.c `FUNCTION GLOBAL "%s" %d "%s" "%s" 0x%x
// %d %u`) for the fake plugin's registrations.
func fnRelistLine(name, help string) string {
	return fmt.Sprintf(`FUNCTION GLOBAL "%s" 10 "%s" "top" 0x13 100 1`, name, help)
}

// fnRelistBursts are the re-lists of fnRelistScenario's first start, in order, as C sends them: the connect-time one
// (or, when the parent refused FUNCTIONS, the first collection's: command-function.c:9 returns before clearing the
// flag, command-begin-set-end-init.c:49-67 does not check FUNCTIONS); the queued FUNCTION_DEL lines first, when
// the parent takes them (nrpc-catalog.c:150-160, dropped otherwise), then C's five built-ins (fnBuiltinRelist:
// registered first, at rrd_init) and the plugin's methods in registration order; a DEL and a re-add between two
// renders give both (fn.registry.readd_keeps_del); a re-added method goes last; an unchanged re-send re-lists too;
// each ends with DynCfg's `config` line (dcConfigLine: localhost's `config`). The exit re-lists nothing, and the next
// session's re-list leaves the exited run's methods out (unavailable): fnRelistJoin(nil) alone.
func fnRelistBursts(del bool) []string {
	a, b := fnRelistLine("difftest-a", "a"), fnRelistLine("difftest-b", "b")
	a2, b2 := fnRelistLine("difftest-a", "a2"), fnRelistLine("difftest-b", "b2")
	delA, delB := []string{`FUNCTION_DEL GLOBAL "difftest-a"`}, []string{`FUNCTION_DEL GLOBAL "difftest-b"`}
	if !del {
		delA, delB = nil, nil
	}
	return []string{fnRelistJoin(nil, a, b), fnRelistJoin(delA, b), fnRelistJoin(delB, b2), fnRelistJoin(nil, b2, a2),
		fnRelistJoin(nil, b2, a2)}
}

// fnRelistJoin is one re-list of a C child (nrpc_catalog_render_global_functions, command-function.c:32-40): the
// queued deletes, the five built-ins, the plugin's methods, then DynCfg's `config` line.
func fnRelistJoin(dels []string, methods ...string) string {
	return strings.Join(slices.Concat(dels, fnBuiltinRelist(), methods, []string{dcConfigLine}), "\n")
}

// fnRelistScenario: the plugin collects in the background from its start (PULSE is off, so its blocks are the only
// collections on localhost and each re-list renders at one of them, on its own thread: a write's lines are never
// split by a render); start 1 registers two methods, and once released, 2.5 s apart (two blocks or more, so each
// change re-lists alone, R61-3): deletes difftest-a; deletes and re-adds difftest-b in one write; re-adds
// difftest-a; re-sends difftest-b unchanged; then waits for the second release to exit (the parent has closed the
// session by then: the run end's obsolete charts, pd.orch.exit.obsolete_charts, are not pushed into a captured
// session). Start 2 collects the same chart, without registering, before the child connects again.
func fnRelistScenario() *plugin.Scenario {
	reg := func(name, help string) string {
		return fmt.Sprintf(`FUNCTION GLOBAL "%s" 10 "%s" "top" "0x13" 100 1`+"\n", name, help)
	}
	del := func(name string) string { return `FUNCTION_DEL GLOBAL "` + name + `"` + "\n" }
	collect := func(background bool) plugin.Step {
		return plugin.Step{Collect: &plugin.Collect{Chart: "difftest.relist", Dims: []string{"x"}, N: 300, Background: background}}
	}
	pause := plugin.Step{SleepMs: 2500}
	return &plugin.Scenario{Starts: []plugin.Start{
		{Steps: []plugin.Step{
			{Emit: reg("difftest-a", "a") + reg("difftest-b", "b")}, collect(true), {WaitFile: "r1"}, pause,
			{Emit: del("difftest-a")}, pause,
			{Emit: del("difftest-b") + reg("difftest-b", "b2")}, pause,
			{Emit: reg("difftest-a", "a2")}, pause,
			{Emit: reg("difftest-b", "b2")}, pause,
			{WaitFile: "r2"}, {Exit: plugin.ExitCode(0)},
		}},
		{Steps: []plugin.Step{collect(false)}},
	}}
}

// fnRelistHold waits until the plugin's second start collected (its first start's run end is over: the methods are
// unavailable and the chart obsolete), then half a second for the daemon to read the block (the chart revived). A
// child that connects again early (before the plugin's changes end) waits here too, up to a minute.
func fnRelistHold(t *testing.T, d *daemon.Daemon) {
	l := plugin.LayoutOf(d.Opts.RunDir)
	if _, ok := l.WaitFor(60*time.Second, func(s [][]plugin.Record) bool {
		return len(s) >= 2 && plugin.Has(s[1], "collected", "")
	}); !ok {
		t.Errorf("%s: the plugin's second start did not collect within 60 s of the child's STREAM request", d.Opts.RunDir)
	}
	time.Sleep(500 * time.Millisecond)
}

// fnRelistVariant is a re-list variant: the parent refuses REPLICATION and `refused`, the child runs the fake plugin
// with PULSE off; the plugin is released once the session's start is out; once its changes are done the parent
// closes the session and the plugin exits. The parent answers the child's next STREAM request only once the plugin's
// second start collects (fnRelistHold), so the second session starts without the exited run's methods and with its
// chart revived. Run 15 (rl2, fn-nodel) showed a C child connecting again before the exit's run end without it:
// session 2 re-listed difftest-b and difftest-a, then carried the chart's obsolete definition.
func fnRelistVariant(name string, refused uint32, relists [][]string) senderVariant {
	return senderVariant{name: name, refused: stream.CapReplication | refused, sessions: 2, plugin: fnRelistScenario(),
		relists: relists,
		hold:    fnRelistHold,
		child:   func(o *daemon.Options) { o.PulseOff = true },
		during: func(t *testing.T, d *daemon.Daemon, s *stream.Session) {
			l := plugin.LayoutOf(d.Opts.RunDir)
			if !s.WaitData(func(b []byte) bool { return bytes.Contains(b, []byte("OVERWRITE labels\n")) }, 30*time.Second) {
				t.Errorf("%s: no host labels within 30 s", d.Opts.RunDir)
				return
			}
			// the connect-time re-list follows the labels (stream-sender.c:174-179)
			time.Sleep(time.Second)
			if err := l.Release("r1"); err != nil {
				t.Error(err)
				return
			}
			if _, ok := l.WaitFor(40*time.Second, func(s [][]plugin.Record) bool {
				return len(s) >= 1 && plugin.Has(s[0], "waiting", "r2")
			}); !ok {
				t.Errorf("%s: the plugin's changes did not end within 40 s", d.Opts.RunDir)
				return
			}
			time.Sleep(time.Second)
			_ = s.Close()
			if err := l.Release("r2"); err != nil {
				t.Error(err)
				return
			}
			if _, ok := l.WaitFor(10*time.Second, func(s [][]plugin.Record) bool {
				return len(s) >= 2 && plugin.Has(s[1], "collected", "")
			}); !ok {
				t.Errorf("%s: the plugin's second start did not collect within 10 s", d.Opts.RunDir)
			}
		}}
}

// dcRelistScenario: the plugin collects in the background from its start (fnRelistScenario's pacing: each change
// re-lists alone, at a collection of its own) and answers DynCfg's calls (dcServe); once released it registers a
// template, a stock job of it (an `enable` echo), a single (another), 2.5 s apart, then deletes the job (never saved:
// the node goes, its method is unregistered), and waits for the stop.
func dcRelistScenario() *plugin.Scenario {
	collect := plugin.Step{Collect: &plugin.Collect{Chart: "difftest.dcrelist", Dims: []string{"x"}, N: 300, Background: true}}
	pause := plugin.Step{SleepMs: 2500}
	return &plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
		dcServe(), collect, {WaitFile: "r1"}, pause,
		{Emit: dcCreateT}, pause, {Emit: dcCreateJ1}, pause, {Emit: dcCreateS}, pause,
		{Emit: "CONFIG " + dcJ1 + " delete\n"}, pause, {WaitFile: "r2"},
	}}}}
}

// dcRelistVariant is a DynCfg re-list variant: one session; the parent refuses REPLICATION and `refused`; the child
// runs dcRelistScenario with PULSE off, released once the session's start is out; the session runs until the
// plugin's changes are done, then senderRun's 15 s.
func dcRelistVariant(name string, refused uint32, relists [][]string) senderVariant {
	return senderVariant{name: name, refused: stream.CapReplication | refused, plugin: dcRelistScenario(), relists: relists,
		child: func(o *daemon.Options) { o.PulseOff = true },
		during: func(t *testing.T, d *daemon.Daemon, s *stream.Session) {
			l := plugin.LayoutOf(d.Opts.RunDir)
			if !s.WaitData(func(b []byte) bool { return bytes.Contains(b, []byte("OVERWRITE labels\n")) }, 30*time.Second) {
				t.Errorf("%s: no host labels within 30 s", d.Opts.RunDir)
				return
			}
			// the connect-time re-list follows the labels (stream-sender.c:174-179)
			time.Sleep(time.Second)
			if err := l.Release("r1"); err != nil {
				t.Error(err)
				return
			}
			if _, ok := l.WaitFor(40*time.Second, func(s [][]plugin.Record) bool {
				return len(s) >= 1 && plugin.Has(s[0], "waiting", "r2")
			}); !ok {
				t.Errorf("%s: the plugin's changes did not end within 40 s", d.Opts.RunDir)
			}
		}}
}

// senderGrandchild is the live child of the retention-change variant's sender child.
var senderGrandchild = stream.HostInfo{Hostname: "sender-grandchild",
	MachineGUID: "5a1e0000-0000-4000-8000-0000000000cf"}

// retentionChange connects a grandchild once the first session's labels are out (so the child's `_is_parent` in
// them does not race it), obsoletes one of its charts, and waits for the second stream path: its localhost entry
// carries the new first time, one retention (60 s) behind the moment it is seen.
func retentionChange(t *testing.T, d *daemon.Daemon, s *stream.Session) {
	if !s.WaitData(func(b []byte) bool { return bytes.Contains(b, []byte("OVERWRITE labels\n")) }, 30*time.Second) {
		t.Errorf("%s: no host labels within 30 s", d.Opts.RunDir)
		return
	}
	g := startLiveChild(t, d, senderGrandchild, nil, map[string][]string{"gc.live": {"d"}, "gc.gone": {"d"}})
	time.Sleep(3 * time.Second)
	g.obsolete("gc.gone", []string{"CHART 'gc.gone' '' 'title' 'units' 'family' 'gc.gone' line 1000 1 'obsolete' maint corpus"},
		nil)
	retentionPaths(t, d.Opts.RunDir, s, d.Opts.Identity.MachineGUID, time.Now(), 0)
}

// streamPathEntry is a stream path entry as the checks read it.
type streamPathEntry struct {
	HostID string   `json:"host_id"`
	Hops   int      `json:"hops"`
	Since  int64    `json:"since"`
	First  int64    `json:"first_time_t"`
	Flags  []string `json:"flags"`
}

// retentionPaths waits up to 210 s after a chart went obsolete (`obsoleted`) for the session's stream path `from`+2
// (the one after path `from`, the last sent before the chart went obsolete) and checks the two: one entry each (the
// agent `guid`, hops 0, the same since), the second's first time at least 30 s above the first's and one retention
// (60 s) behind the moment it was seen. It returns the two entries, ok false when it found them wrong or missing.
// Errors only: the vnode's variant runs it off the test's goroutine.
func retentionPaths(t *testing.T, who string, s *stream.Session, guid string, obsoleted time.Time,
	from int) (first, second streamPathEntry, ok bool) {
	t.Helper()
	if !s.WaitData(func(b []byte) bool { return len(streamPathBlocks(b)) >= from+2 }, 210*time.Second) {
		// the text checks.md quotes for the localhost variant's teeth
		t.Errorf("%s: no second JSON STREAM_PATH within 210 s of the obsolete chart (after path %d)", who, from+1)
		return first, second, false
	}
	seen := time.Now()
	var paths [2]struct {
		Path []streamPathEntry `json:"streaming_path"`
	}
	for i, b := range streamPathBlocks(s.Data())[from : from+2] {
		if err := json.Unmarshal(b, &paths[i]); err != nil {
			t.Errorf("%s: path %d: %v: %s", who, from+i+1, err, b)
			return first, second, false
		}
		if len(paths[i].Path) != 1 || paths[i].Path[0].HostID != guid || paths[i].Path[0].Hops != 0 {
			t.Errorf("%s: path %d: %s", who, from+i+1, b)
			return first, second, false
		}
	}
	first, second = paths[0].Path[0], paths[1].Path[0]
	t.Logf("%s: path %d %s after the obsolete chart, its first time %d s before that, %d s after path %d's",
		who, from+2, seen.Sub(obsoleted).Round(time.Second), seen.Unix()-second.First, second.First-first.First, from+1)
	ok = true
	if second.Since != first.Since {
		t.Errorf("%s: since: %d then %d", who, first.Since, second.Since)
		ok = false
	}
	if second.First-first.First < 30 {
		t.Errorf("%s: first time: %d then %d", who, first.First, second.First)
		ok = false
	}
	if lag := seen.Unix() - second.First; lag < 57 || lag > 63 {
		t.Errorf("%s: path %d's first time is %d s before it was seen, not the 60 s of retention", who, from+2, lag)
		ok = false
	}
	return first, second, ok
}

// TestSenderCapture (checks `stream.sender-capture` and `stream.rchild-transcript`, milestone 7 commits 0, 4 and
// 5, D100, D104.8): a child streams to a recording parent that accepts it in plaintext with the variant's
// capabilities; after the session's start and a few seconds of data the two children's captures are compared, with
// the children's gate and reset records. C against C proves the capture and its normalization stable.
func TestSenderCapture(t *testing.T) {
	bins := binaries(t)
	for _, v := range senderVariants {
		t.Run(v.name, func(t *testing.T) {
			if v.long && os.Getenv("PARITY_LONG") == "" {
				t.Skip("set PARITY_LONG=1")
			}
			sessions := max(v.sessions, 1)
			var caps [2][]capture
			var records [2][]string
			// one at a time: a C child connects before its system-info script ends on a busy host, and sends the
			// fields empty
			for i, role := range []Role{"sender-oracle", "sender-candidate"} {
				caps[i], records[i] = senderRun(t, bins[i], Role(string(role)+"-"+v.name), v, sessions)
			}
			if t.Failed() {
				return
			}
			for n := range sessions {
				compareCaptures(t, "session "+strconv.Itoa(n+1), caps[0][n], caps[1][n])
				if len(caps[0][n].charts) == 0 || len(caps[0][n].data) == 0 {
					t.Errorf("session %d: the oracle's capture has %d charts and %d with data", n+1,
						len(caps[0][n].charts), len(caps[0][n].data))
				}
			}
			diffLines(t, "gate and reset records", records[0], records[1])
			// C's built-ins open every re-list after its deletes (registered first, at rrd_init): each session's first
			// re-list on either side, and session 1 has one whenever the parent takes FUNCTIONS
			for i, role := range []Role{Oracle, Candidate} {
				for n := range sessions {
					rl := caps[i][n].relists
					if len(rl) == 0 {
						if n == 0 && v.refused&stream.CapFunctions == 0 {
							t.Errorf("%s: session 1 has no function re-list", role)
						}
						continue
					}
					if !fnBuiltinsLead(rl[0]) {
						t.Errorf("%s: session %d's first re-list does not start with C's built-ins:\n%s", role, n+1, rl[0])
					}
				}
			}
			for n, want := range v.relists {
				if n < len(caps[0]) && !slices.Equal(caps[0][n].relists, want) {
					t.Errorf("oracle: session %d's function re-lists:\n%s\nwant:\n%s", n+1,
						strings.Join(caps[0][n].relists, "\n--\n"), strings.Join(want, "\n--\n"))
				}
			}
			if v.startHas != "" && !slices.ContainsFunc(caps[0][0].start, regexp.MustCompile(v.startHas).MatchString) {
				t.Errorf("the oracle's first session's start lacks %s:\n%s", v.startHas, strings.Join(caps[0][0].start, "\n"))
			}
			if v.paths > 0 {
				for i, role := range []Role{Oracle, Candidate} {
					n := 0
					for _, l := range caps[i][0].start {
						if l == "JSON STREAM_PATH <payload>" {
							n++
						}
					}
					if n != v.paths {
						t.Errorf("%s: the first session's start holds %d stream paths, not %d", role, n, v.paths)
					}
				}
			}
		})
	}
}

// senderRun runs one child against a recording parent for the variant and returns its sessions' captures and its
// gate and reset records.
func senderRun(t *testing.T, bin string, role Role, v senderVariant, sessions int) ([]capture, []string) {
	var child atomic.Pointer[daemon.Daemon]
	var requests atomic.Int32
	script := func(r stream.Request) stream.Answer {
		if n := requests.Add(1); n > 1 && v.hold != nil {
			if d := child.Load(); d != nil {
				v.hold(t, d)
			}
		}
		a := stream.PlaintextAnswer(r)
		a.Reply = stream.VCaps(r.Caps() &^ (stream.CapsCompression | v.refused))
		return a
	}
	parent, err := stream.StartParent(script)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { parent.Close() })
	var adjust []func(*daemon.Options)
	if v.child != nil {
		adjust = append(adjust, v.child)
	}
	if v.plugin != nil {
		engine, err := plugin.Engine()
		if err != nil {
			t.Fatal(err)
		}
		adjust = append(adjust, func(o *daemon.Options) {
			fnPluginOptions(o, nil, "")
			if _, err := plugin.Install(o.RunDir, engine, *v.plugin); err != nil {
				t.Fatal(err)
			}
		})
	}
	d := senderChild(t, bin, role, parent, v.extra, adjust...)
	child.Store(d)
	var out []capture
	for n := 1; n <= sessions; n++ {
		s := parent.WaitSession(n, 60*time.Second)
		if s == nil {
			t.Fatalf("%s: no STREAM connection %d within 60 s (probes %q)", role, n, parent.Probes())
		}
		if n == 1 && v.during != nil {
			v.during(t, d, s)
		}
		if n == sessions {
			time.Sleep(15 * time.Second)
		}
	}
	if err := d.Stop(); err != nil {
		t.Errorf("stop %s: %v", role, err)
	}
	for n, s := range parent.Sessions()[:sessions] {
		c := parseCapture(s.Request, s.Data())
		if os.Getenv("PARITY_KEEP") == "1" {
			_ = os.WriteFile(filepath.Join(d.Opts.RunDir, "capture-"+strconv.Itoa(n+1)+".txt"), s.Data(), 0o644)
		}
		t.Logf("%s session %d: %d bytes, %d charts, %d with data, %d re-lists, %d other lines", role, n+1, len(s.Data()),
			len(c.charts), len(c.data), len(c.relists), len(c.other))
		out = append(out, c)
	}
	return out, senderRecords(t, d)
}
