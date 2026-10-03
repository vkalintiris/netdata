// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"maps"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/plugin"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// fnTx is a transaction of the fn.child cases: a compact lowercase UUID, as a C parent writes it. Every case uses its
// own numbers: a call id is never reused within a run (the process call table, fn.call.process_wide_ids).
func fnTx(n int) string { return fmt.Sprintf("5a1e00000000400080000000%08x", n) }

// fnTxPrefix starts every transaction the cases write; any other 32-hex id the agents write is a random call id
// (a parent's transaction that is no UUID, nrpc-calls.c:653-654), masked RANDOM.
const fnTxPrefix = "5a1e0000000040008000"

// fnFuture is a plugin's expiry that stays in the future (2100-01-01): kept as given (pluginsd_functions.c:697-701).
const fnFuture = "4102444800"

// fnRegister is the method most cases call. 0x13 is signed-id, same-space and sensitive-data.
const fnRegister = `FUNCTION GLOBAL "difftest-fn" 10 "parity fn" "top" "0x13" 100 1` + "\n"

// fnWait bounds each wait for an answer or a plugin step: C answers in milliseconds, a failing candidate costs this.
const fnWait = 10 * time.Second

// fnCall is a parent's call as C writes it down (pluginsd_functions.c:40-48 is the same shape).
func fnCall(tx string, timeout int, cmd, access, source string) string {
	return fmt.Sprintf(`FUNCTION %s %d "%s" "%s" "%s"`, tx, timeout, cmd, access, source)
}

// fnCase is one fn.child scenario.
type fnCase struct {
	// sc is the fake plugin's scenario; more are other fake plugins (installed before the start), enable their names
	sc     plugin.Scenario
	more   []morePlugin
	enable []string
	// listed is the method whose `FUNCTION GLOBAL` line the parent waits for before it plays ("difftest-fn" when
	// empty)
	listed string
	// refused are the capabilities the parent does not answer, besides compression
	refused uint32
	// debug runs the child at `[logs] level = debug`: the NRPC and hook records are DEBUG (R61-2)
	debug bool
	// play is the parent's script; it returns what it observed on the way (compared between the sides)
	play func(t *testing.T, x *fnSide) []string
	// want are texts the oracle's answers, observations, records or plugin stdin must hold (each by substring);
	// wantNot are texts none of them may hold
	want, wantNot []string
	// starts is how many starts of difftest the oracle must show (1 when 0)
	starts int
	// settle, when set, runs on the test goroutine once both sides played, before this side's stop: what must have
	// happened before the stop cut
	settle func(t *testing.T, x *fnSide)
	// clock renders the answers' wall-clock values (dcClock: DynCfg's trees carry `*_ut` and `agent.now`)
	clock bool
	// records, when set, are more stream-thread records the case compares (besides fnStreamRecordRe's)
	records *regexp.Regexp
	// mask, when set, renders each side's answers and observations before they are compared (the case's own masks)
	mask func([]string) []string
}

// fnSide is one agent's run of a case: its child, its parent, the session the case calls through (localhost's
// unless the case switches) and the fake plugins.
type fnSide struct {
	role Role
	// begin is when the side started: the earliest expiry an answer may carry
	begin  time.Time
	d      *daemon.Daemon
	parent *stream.Parent
	s      *stream.Session
	l      plugin.Layout            // difftest
	more   map[string]plugin.Layout // the other fake plugins, by file
}

// send writes lines down to the child.
func (x *fnSide) send(t *testing.T, lines ...string) {
	t.Helper()
	if err := x.s.Send(lines...); err != nil {
		t.Errorf("%s: send: %v", x.role, err)
	}
}

// answered waits until the child sent up n whole answers for tx.
func (x *fnSide) answered(t *testing.T, tx string, n int) bool {
	t.Helper()
	if x.s.WaitData(func(b []byte) bool { return fnCountAnswers(b, tx) >= n }, fnWait) {
		return true
	}
	t.Errorf("%s: %d answers for %s within %v: %d", x.role, n, tx, fnWait, fnCountAnswers(x.s.Data(), tx))
	return false
}

// listed waits until the child listed the method upstream (a re-list's `FUNCTION GLOBAL "<name>" ` line).
func (x *fnSide) listed(t *testing.T, name string) bool {
	t.Helper()
	line := []byte("\nFUNCTION GLOBAL \"" + name + "\" ")
	if x.s.WaitData(func(b []byte) bool { return bytes.Contains(b, line) }, 30*time.Second) {
		return true
	}
	t.Errorf("%s: %q not listed upstream within 30 s", x.role, name)
	return false
}

// step waits until a fake plugin's starts satisfy ok.
func (x *fnSide) step(t *testing.T, l plugin.Layout, what string, ok func([][]plugin.Record) bool) bool {
	t.Helper()
	if _, done := l.WaitFor(fnWait, ok); done {
		return true
	}
	t.Errorf("%s: %s within %v", x.role, what, fnWait)
	return false
}

// release creates a WaitFile step's file.
func (x *fnSide) release(t *testing.T, l plugin.Layout, name string) {
	t.Helper()
	if err := l.Release(name); err != nil {
		t.Errorf("%s: %v", x.role, err)
	}
}

// stdin is what start n (from 1) of a fake plugin read so far.
func (x *fnSide) stdin(l plugin.Layout, n int) string {
	starts, _ := l.Starts()
	if len(starts) < n {
		return ""
	}
	return plugin.ViewOf(starts[n-1]).Stdin
}

// fnMatched holds once start n (from 1) matched the Expect step `name`.
func fnMatched(n int, name string) func([][]plugin.Record) bool {
	return func(s [][]plugin.Record) bool {
		if len(s) < n {
			return false
		}
		_, ok := plugin.Matched(s[n-1], name)
		return ok
	}
}

// fnCountAnswers counts the whole FUNCTION_RESULT spans a child sent up for tx (the transaction as the parent wrote it).
func fnCountAnswers(b []byte, tx string) int {
	begin := []byte("\nFUNCTION_RESULT_BEGIN \"" + tx + "\" ")
	end := []byte("\nFUNCTION_RESULT_END\n")
	n := 0
	for {
		i := bytes.Index(b, begin)
		if i < 0 {
			return n
		}
		b = b[i+len(begin):]
		j := bytes.Index(b, end)
		if j < 0 {
			return n
		}
		n++
		b = b[j:]
	}
}

var (
	fnBeginRe    = regexp.MustCompile(`^FUNCTION_RESULT_BEGIN "([^"]*)" (-?\d+) "([^"]*)" (-?\d+)$`)
	fnProgressRe = regexp.MustCompile(`^FUNCTION_PROGRESS '([^']*)' (\d+) (\d+)$`)
	fnIDRe       = regexp.MustCompile(`\b[0-9a-f]{32}\b`)
	// the stream thread's records about calls: nRPC's, the transport's hooks (they run on the caller's thread), the
	// progress extension's (no prefix, functions_evloop.h:103-107) and the parser's unprefixed ones
	fnStreamRecordRe = regexp.MustCompile(`msg="(NRPC: |PLUGINSD: |got a |Extending function timeout|Received PROGRESS update)`)
	// an unavailable method's DEBUG record names its serving thread (nrpc-registry.c:860-867)
	fnServingTidRe = regexp.MustCompile(`tid: \d+`)
	// a record logged while a block ends names the block's END line, with the wall-clock second it collected
	// (C vs C run 15: 1790904892 against 1790904894 for the vnode's "streaming is ready")
	fnEndTimeRe = regexp.MustCompile(`request="'END' '\d+'`)
)

// recordBefore tells whether a log line was written before stop (always when stop is zero, or the line has no time):
// what an agent logged before its stop began (R61-4).
func recordBefore(line string, stop time.Time) bool {
	if stop.IsZero() {
		return true
	}
	m := recordTimeRe.FindStringSubmatch(line)
	if m == nil {
		return true
	}
	at, err := time.Parse(time.RFC3339Nano, m[1])
	return err != nil || at.Before(stop)
}

// fnMaskRecord masks what differs between two runs of the same agent in a call or plugin record: random call ids,
// each side's parent's port (rchildRecords' masks: the stream thread's `dst_port=`, a vnode's "streaming enabled
// (to '127.0.0.1:<port>'"), a block's wall-clock second, an unavailable method's serving thread id.
func fnMaskRecord(l string) string {
	l = anyLocalPortRe.ReplaceAllString(l, "127.0.0.1${1}P")
	l = anyDstPortRe.ReplaceAllString(l, " dst_port=P")
	l = fnEndTimeRe.ReplaceAllString(l, `request="'END' 'T'`)
	l = fnServingTidRe.ReplaceAllString(l, "tid: N")
	// the Rust agent masks the stream API key (D31, D34)
	l = hsHostKeyRe.ReplaceAllString(l, "with api key 'K'")
	return fnMaskIDs(l)
}

// fnMaskIDs masks the 32-hex ids no case wrote: random call ids.
func fnMaskIDs(s string) string {
	return fnIDRe.ReplaceAllStringFunc(s, func(id string) string {
		if strings.HasPrefix(id, fnTxPrefix) {
			return id
		}
		return "RANDOM"
	})
}

// fnLine is a line a child sent up, with the arrival of the chunk it starts in.
type fnLine struct {
	text string
	at   time.Time
}

// fnLines are a session's whole lines that arrived before the stop (a line the stop cut is left out, R61-4).
func fnLines(chunks []stream.Chunk, stop time.Time) []fnLine {
	var out []fnLine
	var part []byte
	var partAt time.Time
	for _, c := range chunks {
		if !c.At.Before(stop) {
			break
		}
		data := c.Data
		for len(data) > 0 {
			if len(part) == 0 {
				partAt = c.At
			}
			i := bytes.IndexByte(data, '\n')
			if i < 0 {
				part = append(part, data...)
				break
			}
			out = append(out, fnLine{text: string(append(part, data[:i]...)), at: partAt})
			part, data = nil, data[i+1:]
		}
	}
	return out
}

// fnExpires renders an answer's expiry. An error body's is the call's realtime second + 1 (json-c-parser-inline.c
// :49-50): NOW+1 when it is 1 after the arrival second of the chunk holding the BEGIN line, or 0 after it while that
// second is in its first half (the call's second ended on the way). Another expiry within the run [lo, hi] is
// rendered as its delta (NOW+0 for a candidate's `now`), any other as written (the plugin's constants).
func fnExpires(v string, at time.Time, lo, hi int64) string {
	e, err := strconv.ParseInt(v, 10, 64)
	if err != nil || e < lo || e > hi {
		return v
	}
	d := e - at.Unix()
	if d == 1 || d == 0 && at.Nanosecond() < 500_000_000 {
		return "NOW+1"
	}
	return fmt.Sprintf("NOW%+d", d)
}

// fnUpstream are the function answers a child sent up before the stop, per transaction (as the line names it) in
// arrival order: each FUNCTION_RESULT_BEGIN..FUNCTION_RESULT_END span as `RESULT <code> "<content type>" <expires>
// [<body lines>]` (fnExpires; the lines quoted, the framing's newline before the END making the last one, so an empty
// body is [""] and a missing framing newline []), and each `FUNCTION_PROGRESS '<tx>' d a` line as `PROGRESS d a`;
// then one line with the transactions of all of them in arrival order. Lines between BEGIN and the exact END line are
// body, whatever they look like.
func fnUpstream(lines []fnLine, lo, hi int64) []string {
	per := map[string][]string{}
	var order []string
	var tx, head string
	var body []string
	in := false
	for _, l := range lines {
		if in {
			if l.text == "FUNCTION_RESULT_END" {
				per[tx] = append(per[tx], fmt.Sprintf("%s %q", head, body))
				order = append(order, tx)
				in = false
			} else {
				body = append(body, l.text)
			}
			continue
		}
		if m := fnBeginRe.FindStringSubmatch(l.text); m != nil {
			tx, body, in = m[1], nil, true
			head = fmt.Sprintf("RESULT %s %q %s", m[2], m[3], fnExpires(m[4], l.at, lo, hi))
		} else if m := fnProgressRe.FindStringSubmatch(l.text); m != nil {
			per[m[1]] = append(per[m[1]], "PROGRESS "+m[2]+" "+m[3])
			order = append(order, m[1])
		}
	}
	var out []string
	for _, k := range slices.Sorted(maps.Keys(per)) {
		for _, e := range per[k] {
			out = append(out, fnMaskIDs(k+": "+e))
		}
	}
	return append(out, fnMaskIDs("arrival order: "+strings.Join(order, " ")))
}

// fnViews are a fake plugin's starts up to the stop, one line per item: the arguments, each line read on stdin
// (quoted, random ids masked), the steps and how it ended, the rules a Serve step answered with; and each start's stdin
// whole, for the oracle's guards.
func fnViews(name string, starts [][]plugin.Record, stop time.Time) (lines, stdin []string) {
	for i, s := range starts {
		v := plugin.ViewOf(plugin.Before(s, stop))
		head := fmt.Sprintf("%s start %d:", name, i+1)
		lines = append(lines, fmt.Sprintf("%s args %q", head, v.Args))
		in := fnMaskIDs(v.Stdin)
		for _, l := range strings.SplitAfter(in, "\n") {
			if l != "" {
				lines = append(lines, head+" stdin "+strconv.Quote(l))
			}
		}
		lines = append(lines, fmt.Sprintf("%s steps %q eof %t end %q", head, v.Steps, v.EOF, v.End))
		if len(v.Served) > 0 {
			lines = append(lines, fmt.Sprintf("%s served %q", head, v.Served))
		}
		stdin = append(stdin, in)
	}
	return lines, stdin
}

// fnStreamRecords are the stream thread's records about calls (thread STREAM[n], fnStreamRecordRe) written before
// the stop, in file order, normalized.
func fnStreamRecords(t *testing.T, d *daemon.Daemon, stop time.Time) []string {
	t.Helper()
	return fnStreamRecordsWith(t, d, stop, nil)
}

// fnStreamRecordsWith is fnStreamRecords with the stream thread's records matching extra too (when set).
func fnStreamRecordsWith(t *testing.T, d *daemon.Daemon, stop time.Time, extra *regexp.Regexp) []string {
	t.Helper()
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if strings.HasPrefix(threadOf(l), "STREAM[") && (fnStreamRecordRe.MatchString(l) || extra != nil && extra.MatchString(l)) &&
			recordBefore(l, stop) {
			out = append(out, fnMaskRecord(normalizeLog(l, d.Opts.RunDir, "")))
		}
	}
	return out
}

// fnPluginOptions runs the fake plugins as the plugin checks do (pluginsOptions: the oracle's stock plugins.d first,
// the run directory's second, a scan every second, `[plugin:difftest]` with an update every of 1), with `enable`
// enabled too and `logs` as the [logs] section; the child's other options (its database, PULSE) stay.
func fnPluginOptions(o *daemon.Options, enable []string, logs string) {
	po := pluginsOptions(1, nil, enable, logs)
	o.PluginsDir, o.PluginsExtra, o.ConfExtra, o.LogsExtra = po.PluginsDir, po.PluginsExtra, po.ConfExtra, po.LogsExtra
}

// fnResult is what one side's run of a case left.
type fnResult struct {
	ran     bool
	answers []string
	obs     []string
	views   []string
	stdin   []string
	starts  int
	// classes are the fake plugin threads' records (pluginLogClasses' PD classes); stream the stream thread's
	classes map[string][]string
	stream  []string
}

// fnStart starts one agent's side of a case, on the test goroutine: its recording parent (plaintext, the case's
// capabilities refused) and its child with the fake plugins installed; nil when it could not start.
func fnStart(t *testing.T, bin string, role Role, engine string, c fnCase) *fnSide {
	parent, err := stream.StartParent(func(r stream.Request) stream.Answer {
		a := stream.PlaintextAnswer(r)
		a.Reply = stream.VCaps(r.Caps() &^ (stream.CapsCompression | c.refused))
		return a
	})
	if err != nil {
		t.Errorf("%s: %v", role, err)
		return nil
	}
	t.Cleanup(func() { parent.Close() })
	logs := ""
	if c.debug {
		logs = "    level = debug\n"
	}
	x := &fnSide{role: role, begin: time.Now(), parent: parent, more: map[string]plugin.Layout{}}
	var installErr error
	x.d = senderChild(t, bin, role, parent, "", func(o *daemon.Options) {
		fnPluginOptions(o, c.enable, logs)
		if x.l, err = plugin.Install(o.RunDir, engine, c.sc); err != nil {
			installErr = err
		}
		for _, m := range c.more {
			l, err := plugin.InstallAs(o.RunDir, filepath.Join(o.RunDir, m.dir), m.file, engine, m.sc)
			if err != nil {
				installErr = err
			}
			x.more[m.file] = l
		}
	})
	if installErr != nil {
		t.Errorf("%s: install: %v", role, installErr)
		return nil
	}
	return x
}

// fnPlay waits for the child's session and the listing of the case's method, then plays the case; it runs on a
// goroutine of its own (t.Errorf only), and tells whether the case played.
func fnPlay(t *testing.T, x *fnSide, c fnCase) ([]string, bool) {
	if x.s = x.parent.WaitSessionFor(x.d.Opts.Identity.MachineGUID, 1, 60*time.Second); x.s == nil {
		t.Errorf("%s: no STREAM session within 60 s", x.role)
		return nil, false
	}
	listed := c.listed
	if listed == "" {
		listed = "difftest-fn"
	}
	if !x.listed(t, listed) {
		return nil, false
	}
	return c.play(t, x), true
}

// fnCollect stops a side and takes what it left, on the test goroutine: what reaches the parent once the stop began
// is a race (C stops collectors and streaming together, daemon-shutdown.c:227), as are the plugin thread's records
// of its kill, so the answers, the plugin starts and the records end at the stop (R61-4); records are the case's more
// stream-thread records.
func fnCollect(t *testing.T, x *fnSide, records *regexp.Regexp) fnResult {
	var res fnResult
	stop := time.Now()
	if err := x.d.Stop(); err != nil {
		t.Errorf("%s: stop: %v", x.role, err)
	}
	if x.s != nil {
		res.answers = fnUpstream(fnLines(x.s.Chunks(), stop), x.begin.Unix()-5, stop.Unix()+5)
	}
	starts, err := x.l.Starts()
	if err != nil {
		t.Errorf("%s: %v", x.role, err)
	}
	res.starts = len(starts)
	res.views, res.stdin = fnViews(plugin.Name, starts, stop)
	for _, file := range slices.Sorted(maps.Keys(x.more)) {
		more, err := x.more[file].Starts()
		if err != nil {
			t.Errorf("%s: %v", x.role, err)
		}
		v, in := fnViews(file, more, stop)
		res.views, res.stdin = append(res.views, v...), append(res.stdin, in...)
	}
	res.classes = map[string][]string{}
	for class, lines := range pluginLogClassesBefore(t, x.d, stop) {
		if strings.Contains(class, "thread=PD[") {
			for _, l := range lines {
				res.classes[class] = append(res.classes[class], fnMaskRecord(l))
			}
		}
	}
	res.stream = fnStreamRecordsWith(t, x.d, stop, records)
	return res
}

// runFnCases plays each case on a C child and a Rust child at once, each with its own parent, then checks the
// oracle's guards and compares: the answers per transaction, the observations, every fake plugin start (stdin byte
// for byte), the fake plugin threads' records per class in file order and the stream thread's call records in order.
func runFnCases(t *testing.T, cases map[string]fnCase) {
	bins := binaries(t)
	engine, err := plugin.Engine()
	if err != nil {
		t.Fatal(err)
	}
	for _, name := range slices.Sorted(maps.Keys(cases)) {
		c := cases[name]
		t.Run(name, func(t *testing.T) {
			// the daemons start and stop on the test goroutine (FailNow is only supported there), the two sides
			// play at once
			var sides [2]*fnSide
			for i, role := range []Role{Oracle, Candidate} {
				sides[i] = fnStart(t, bins[i], role, engine, c)
			}
			var obs [2][]string
			var ran [2]bool
			var wg sync.WaitGroup
			for i, x := range sides {
				if x == nil {
					continue
				}
				wg.Add(1)
				go func() {
					defer wg.Done()
					obs[i], ran[i] = fnPlay(t, x, c)
				}()
			}
			wg.Wait()
			var res [2]fnResult
			for i, x := range sides {
				if x == nil {
					continue
				}
				if ran[i] && c.settle != nil {
					c.settle(t, x)
				}
				res[i] = fnCollect(t, x, c.records)
				res[i].obs, res[i].ran = obs[i], ran[i]
				if c.clock {
					now := time.Now()
					res[i].answers = dcClock(res[i].answers, now.Add(-10*time.Minute), now.Add(time.Minute))
				}
				if c.mask != nil {
					res[i].answers, res[i].obs = c.mask(res[i].answers), c.mask(res[i].obs)
				}
			}
			if !res[0].ran {
				t.Fatal("oracle: the case did not play")
			}
			o := res[0]
			if want := max(c.starts, 1); o.starts != want {
				t.Errorf("oracle: %d starts of %s, want %d", o.starts, plugin.Name, want)
			}
			hay := slices.Concat(o.answers, o.obs, o.stdin, o.stream)
			for _, lines := range o.classes {
				hay = append(hay, lines...)
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
			diffLines(t, "upstream answers", o.answers, res[1].answers)
			diffLines(t, "observations", o.obs, res[1].obs)
			diffLines(t, "plugin starts", o.views, res[1].views)
			diffLines(t, "stream thread call records", o.stream, res[1].stream)
			for _, class := range slices.Sorted(maps.Keys(o.classes)) {
				diffLines(t, class, o.classes[class], res[1].classes[class])
			}
			for class := range res[1].classes {
				if _, ok := o.classes[class]; !ok {
					t.Errorf("candidate only: %s: %q", class, res[1].classes[class])
				}
			}
			t.Logf("oracle answers:\n%s\nobservations:\n%s\nplugin starts:\n%s\nstream thread records:\n%s",
				strings.Join(o.answers, "\n"), strings.Join(o.obs, "\n"), strings.Join(o.views, "\n"), strings.Join(o.stream, "\n"))
		})
	}
}

// TestFnChild (check `fn.child`, M8 commit 5, D147): a scripted parent calls a C child's and a Rust child's
// fake-plugin functions (the child runs a parent's FUNCTION as a no-wait call into the plugin, stream-sender-execute.c
// :59-104). Compared per case: the answers the child sends up, grouped per transaction in arrival order (spans
// parsed whole, an error body's expiry masked), the plugin's stdin byte for byte with its steps, the fake plugin
// threads' records and the stream thread's call records. Calls go one at a time, each after the previous answer.
func TestFnChild(t *testing.T) {
	runFnCases(t, fnCases())
}

// fnAnswer is the oracle's answer line for tx (a substring of fnUpstream's): its body's lines, the last one the
// framing's.
func fnAnswer(tx, code, ct, expires, body string) string {
	return fmt.Sprintf("%s: RESULT %s %q %s %q", tx, code, ct, expires, strings.Split(body, "\n"))
}

// fnError is an nRPC error answer (json-c-parser-inline.c:43-53).
func fnError(tx string, code int, msg string) string {
	return fnAnswer(tx, strconv.Itoa(code), "application/json", "NOW+1",
		fmt.Sprintf(`{"status":%d,"errorMessage":"%s"}`, code, msg))
}

// fnAt sleeps until d after t0.
func fnAt(t0 time.Time, d time.Duration) { time.Sleep(time.Until(t0.Add(d))) }

func fnCases() map[string]fnCase {
	expect := plugin.ExpectFunction
	emit := func(s string) plugin.Step { return plugin.Step{Emit: s} }
	hang := plugin.Step{Hang: true}
	notRunning := "The plugin that registered this feature, is not currently running."
	exited := "The plugin that was servicing this request, exited before responding."
	cases := map[string]fnCase{}

	// two answers with constant expiries (a future one kept, a past one sent as 0: R61-11), a command with words
	// stripped down to the method (the plugin gets it whole), a source with a control byte (sanitized), old-role
	// access (member = 0x1b), a timeout of 0 (10 s, stream-sender-execute.c:72-73), two unknown methods (404), a
	// duplicate while the first is pending (400 and a NOTICE, nrpc-calls.c:715-731), and the `json` alias
	{
		a, b, n1, n2, d := fnTx(0x101), fnTx(0x102), fnTx(0x103), fnTx(0x104), fnTx(0x105)
		cases["ok"] = fnCase{
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				emit(fnRegister),
				expect("a"), emit(plugin.Result("{{a}}", "200", "application/json", fnFuture, "{\"ok\":1}\n")),
				expect("b"), emit(plugin.Result("{{b}}", "200", "text/plain", "1", "line one\nline two\n")),
				expect("d"), {SleepMs: 1000}, emit(plugin.Result("{{d}}", "200", "json", "0", "{}\n")),
			}}}},
			play: func(t *testing.T, x *fnSide) []string {
				x.send(t, fnCall(a, 2, "difftest-fn a  b", "0x13", "method=parity,user=\x01x"))
				x.answered(t, a, 1)
				x.send(t, fnCall(b, 0, "difftest-fn", "member", "src"))
				x.answered(t, b, 1)
				x.send(t, fnCall(n1, 2, "nope x", "0x13", "src"), fnCall(n2, 2, "difftest-fnx y", "0x13", "src"))
				x.answered(t, n1, 1)
				x.answered(t, n2, 1)
				x.send(t, fnCall(d, 2, "difftest-fn d", "0x13", "src"), fnCall(d, 2, "difftest-fn d", "0x13", "src"))
				x.answered(t, d, 2)
				return nil
			},
			want: []string{
				fnAnswer(a, "200", "application/json", fnFuture, "{\"ok\":1}\n"),
				fnAnswer(b, "200", "text/plain", "0", "line one\nline two\n"),
				fnError(n1, 404, "This feature is not available on this host at this time."),
				fnError(n2, 404, "This feature is not available on this host at this time."),
				fnError(d, 400, "Duplicate transaction."),
				fnAnswer(d, "200", "application/json", "0", "{}\n"),
				"FUNCTION " + a + ` 2 "difftest-fn a`,
				"FUNCTION " + b + ` 10 "difftest-fn" "0x1b" "src"`,
				"NRPC: duplicate call_id '" + d + "', method: 'difftest-fn d'",
			},
		}
	}

	// what the plugin's RESULT_BEGIN words become (pluginsd_functions.c:669-713): status 0 and garbage are 591; an
	// unknown format is text/plain, an empty one (quoted) and a missing expiry are logged and the rest goes on, an
	// alias maps to its type; the plugin's own error body passes verbatim (fn.err.shape_plugin); a body line that
	// only starts with the END keyword is body, `=` ends the keyword (isspace_pluginsd); an empty body
	{
		tx := func(i int) string { return fnTx(0x200 + i) }
		answers := []string{
			"FUNCTION_RESULT_BEGIN {{c}} 0 application/json 0\n{\"x\":1}\nFUNCTION_RESULT_END\n",
			"FUNCTION_RESULT_BEGIN {{c}} abc text/plain 0\nabc\nFUNCTION_RESULT_END\n",
			"FUNCTION_RESULT_BEGIN {{c}} 200 application/x-nope 0\nnope\nFUNCTION_RESULT_END\n",
			"FUNCTION_RESULT_BEGIN {{c}} 200 \"\" 0\nempty format\nFUNCTION_RESULT_END\n",
			"FUNCTION_RESULT_BEGIN {{c}} 200 text/html\n<p>hi</p>\nFUNCTION_RESULT_END\n",
			"FUNCTION_RESULT_BEGIN {{c}} 404 application/json 0\n{\"status\":404,\"error_message\":\"plugin says no\"}\nFUNCTION_RESULT_END\n",
			"FUNCTION_RESULT_BEGIN {{c}} 200 prometheus 0\na\nFUNCTION_RESULT_ENDX\nFUNCTION_RESULT_END=done\n",
			"FUNCTION_RESULT_BEGIN {{c}} 200 text/plain " + fnFuture + "\nFUNCTION_RESULT_END\n",
		}
		var steps []plugin.Step
		steps = append(steps, emit(fnRegister))
		for _, a := range answers {
			steps = append(steps, expect("c"), emit(a))
		}
		cases["codes"] = fnCase{
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: steps}}},
			play: func(t *testing.T, x *fnSide) []string {
				for i := range answers {
					x.send(t, fnCall(tx(i), 2, fmt.Sprintf("difftest-fn c%d", i), "0x13", "src"))
					x.answered(t, tx(i), 1)
				}
				return nil
			},
			want: []string{
				fnAnswer(tx(0), "591", "application/json", "0", "{\"x\":1}\n"),
				fnAnswer(tx(1), "591", "text/plain", "0", "abc\n"),
				fnAnswer(tx(2), "200", "text/plain", "0", "nope\n"),
				fnAnswer(tx(3), "200", "text/plain", "0", "empty format\n"),
				fnAnswer(tx(4), "200", "text/html", "0", "<p>hi</p>\n"),
				fnAnswer(tx(5), "404", "application/json", "0", "{\"status\":404,\"error_message\":\"plugin says no\"}\n"),
				fnAnswer(tx(6), "200", "text/plain", "0", "a\nFUNCTION_RESULT_ENDX\n"),
				fnAnswer(tx(7), "200", "text/plain", fnFuture, ""),
				"got a FUNCTION_RESULT_BEGIN without providing the required data (key = '" + tx(3) +
					"', status = '200', format = '', expires = '0').",
				"(key = '" + tx(4) + "', status = '200', format = 'text/html', expires = '(unset)').",
			},
		}
	}

	// FUNCTION_PAYLOAD (R61-9): a two-line JSON body (each line keeps its newline, then a blank line before the end,
	// pluginsd_functions.c:24-38); an empty body is a plain FUNCTION line; a missing and an unknown content type are
	// text/plain; a body of one empty line
	{
		p1, p2, p3, p4 := fnTx(0x301), fnTx(0x302), fnTx(0x303), fnTx(0x304)
		ok := func(name string) plugin.Step {
			return emit(plugin.Result("{{"+name+"}}", "200", "text/plain", "0", name+"\n"))
		}
		cases["payload"] = fnCase{
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				emit(fnRegister),
				plugin.ExpectPayload("p1"), ok("p1"), expect("p2"), ok("p2"),
				plugin.ExpectPayload("p3"), ok("p3"), plugin.ExpectPayload("p4"), ok("p4"),
			}}}},
			play: func(t *testing.T, x *fnSide) []string {
				x.send(t, `FUNCTION_PAYLOAD `+p1+` 2 "difftest-fn p" "0x13" "src" "application/json"`, `{"a":1}`, `{"b":2}`,
					`FUNCTION_PAYLOAD_END`)
				x.answered(t, p1, 1)
				x.send(t, `FUNCTION_PAYLOAD `+p2+` 2 "difftest-fn e" "0x13" "src" "application/json"`, `FUNCTION_PAYLOAD_END`)
				x.answered(t, p2, 1)
				x.send(t, `FUNCTION_PAYLOAD `+p3+` 2 "difftest-fn u" "0x13" "src"`, `x=1`, `FUNCTION_PAYLOAD_END`)
				x.answered(t, p3, 1)
				x.send(t, `FUNCTION_PAYLOAD `+p4+` 2 "difftest-fn v" "0x13" "src" "application/x-nope"`, ``,
					`FUNCTION_PAYLOAD_END`)
				x.answered(t, p4, 1)
				return nil
			},
			want: []string{
				"FUNCTION_PAYLOAD " + p1 + " 2 \"difftest-fn p\" \"0x13\" \"src\" \"application/json\"\n{\"a\":1}\n{\"b\":2}\n\nFUNCTION_PAYLOAD_END\n",
				"FUNCTION " + p2 + " 2 \"difftest-fn e\" \"0x13\" \"src\"\n",
				"FUNCTION_PAYLOAD " + p3 + " 2 \"difftest-fn u\" \"0x13\" \"src\" \"text/plain\"\nx=1\n\nFUNCTION_PAYLOAD_END\n",
				"FUNCTION_PAYLOAD " + p4 + " 2 \"difftest-fn v\" \"0x13\" \"src\" \"text/plain\"\n\n\nFUNCTION_PAYLOAD_END\n",
				fnAnswer(p4, "200", "text/plain", "0", "p4\n"),
			},
		}
	}

	// authorization (nrpc-calls.c:540-580) with the parent's access words (old roles mapped,
	// http-access.c:146-160): anonymous (412 SSO), signed-id (403 space), signed-id and same-space (403 the missing
	// permission's name), empty and garbage (412), admin (0x7b) and all bits (0x7ff) run; a `__` method and one
	// tagged hidden are RESTRICTED and run for a parent (allow_restricted, R61 fn.access.child_trust); a commercial
	// method refuses a non-commercial caller
	{
		ax := func(i int) string { return fnTx(0x400 + i) }
		ok := func(name string) plugin.Step {
			return emit(plugin.Result("{{"+name+"}}", "200", "text/plain", "0", name+"\n"))
		}
		register := fnRegister +
			`FUNCTION GLOBAL "__difftest-hidden" 10 "hidden" "top" "0x0" 100 1` + "\n" +
			`FUNCTION GLOBAL "difftest-tagged" 10 "tagged" "hidden" "0x8" 100 1` + "\n" +
			`FUNCTION GLOBAL "difftest-paid" 10 "paid" "top" "0x4" 100 1` + "\n"
		sso := "You need to be authenticated via Netdata Cloud Single-Sign-On (SSO) to access this feature. " +
			"Sign-in on this dashboard, or access your Netdata via https://app.netdata.cloud."
		cases["access"] = fnCase{
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				emit(register), expect("x5"), ok("x5"), expect("x6"), ok("x6"), expect("x8"), ok("x8"),
				expect("x9"), ok("x9"),
			}}}},
			listed: "difftest-paid",
			play: func(t *testing.T, x *fnSide) []string {
				calls := []struct {
					cmd, access string
				}{
					{"difftest-fn", "0x8"}, {"difftest-fn", "0x1"}, {"difftest-fn", "0x3"}, {"difftest-fn", ""},
					{"difftest-fn", "admin"}, {"difftest-fn", "0xFFFFFFFF"}, {"difftest-fn", "zz"},
					{"__difftest-hidden", "0x0"}, {"difftest-tagged", "0x8"}, {"difftest-paid", "0x3"},
				}
				for i, c := range calls {
					x.send(t, fnCall(ax(i+1), 2, c.cmd, c.access, "src"))
					x.answered(t, ax(i+1), 1)
				}
				return nil
			},
			want: []string{
				fnError(ax(1), 412, sso),
				fnError(ax(2), 403, "You need to login to the Netdata Cloud space this agent is claimed to, to access this feature."),
				fnError(ax(3), 403, "This feature requires additional permissions: sensitive-data."),
				fnError(ax(4), 412, sso),
				"FUNCTION " + ax(5) + ` 2 "difftest-fn" "0x7b" "src"`,
				"FUNCTION " + ax(6) + ` 2 "difftest-fn" "0x7ff" "src"`,
				fnError(ax(7), 412, sso),
				"FUNCTION " + ax(8) + ` 2 "__difftest-hidden" "0x0" "src"`,
				fnAnswer(ax(9), "200", "text/plain", "0", "x9\n"),
				fnError(ax(10), 403, "This feature is only available for commercial users and supporters of Netdata. "+
					"To use it, please upgrade your space. Thank you for supporting Netdata."),
			},
		}
	}

	// transaction spellings (R61-10): a dashed uppercase UUID runs under its compact lowercase id (the plugin's
	// line) and the answer echoes it as the parent wrote it, but a cancel or progress by that spelling misses (an
	// exact lookup, nrpc-calls.c:807,821) while the compact spelling hits; a transaction that is no UUID runs under a
	// random id (masked) and still gets its answer; a UUID with trailing garbage (uuid_parse_flexi)
	{
		t1, t1c, t2, t3 := "5A1E0000-0000-4000-8000-00000000F001", fnTx(0xf001), "tx-not-a-uuid", fnTx(0xf003)+"xyz"
		ok := func(name string) plugin.Step {
			return emit(plugin.Result("{{"+name+"}}", "200", "text/plain", "0", name+"\n"))
		}
		cases["tx"] = fnCase{
			debug: true,
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				emit(fnRegister), expect("t1"), {WaitFile: "go"}, ok("t1"), expect("t2"), ok("t2"), expect("t3"), ok("t3"),
			}}}},
			play: func(t *testing.T, x *fnSide) []string {
				x.send(t, fnCall(t1, 10, "difftest-fn t1", "0x13", "src"))
				x.step(t, x.l, "the plugin did not get t1", fnMatched(1, "t1"))
				x.send(t, "FUNCTION_CANCEL "+t1, "FUNCTION_PROGRESS "+t1)
				time.Sleep(300 * time.Millisecond)
				x.send(t, "FUNCTION_CANCEL "+t1c)
				time.Sleep(300 * time.Millisecond)
				x.release(t, x.l, "go")
				x.answered(t, t1, 1)
				x.send(t, fnCall(t2, 10, "difftest-fn t2", "0x13", "src"))
				x.answered(t, t2, 1)
				x.send(t, fnCall(t3, 10, "difftest-fn t3", "0x13", "src"))
				x.answered(t, t3, 1)
				return nil
			},
			want: []string{
				fnAnswer(t1, "200", "text/plain", "0", "t1\n"),
				fnAnswer(t2, "200", "text/plain", "0", "t2\n"),
				"FUNCTION " + t1c + ` 10 "difftest-fn t1"`,
				"FUNCTION_CANCEL " + t1c + "\n",
				"NRPC: received a CANCEL request for call_id '" + t1 + "', but the call_id is not running.",
				"NRPC: received a PROGRESS request for call_id '" + t1 + "', but the call_id is not running.",
				`FUNCTION RANDOM 10 "difftest-fn t2"`,
			},
		}
	}

	// a parent's cancels (fn.call.cancel) at the debug level: the first is written to the plugin, a second is
	// "already cancelled", one for a call nobody made and one after the answer are "not running"; a progress after
	// the cancel still reaches the plugin (fn.call.progress_after_cancel); the plugin answers after the cancel, so the
	// span closes (R61-2)
	{
		a, u := fnTx(0x501), fnTx(0x5ff)
		cases["cancel"] = fnCase{
			debug: true,
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				emit(fnRegister), expect("a"), plugin.ExpectCancel("x"), plugin.ExpectProgress("y"), {SleepMs: 200},
				emit(plugin.Result("{{a}}", "200", "text/plain", "0", "cancelled\n")),
			}}}},
			play: func(t *testing.T, x *fnSide) []string {
				x.send(t, fnCall(a, 10, "difftest-fn c", "0x13", "src"))
				x.step(t, x.l, "the plugin did not get the call", fnMatched(1, "a"))
				x.send(t, "FUNCTION_CANCEL "+a, "FUNCTION_CANCEL "+a, "FUNCTION_PROGRESS "+a, "FUNCTION_CANCEL "+u)
				x.answered(t, a, 1)
				x.send(t, "FUNCTION_CANCEL "+a)
				time.Sleep(500 * time.Millisecond)
				return nil
			},
			want: []string{
				fnAnswer(a, "200", "text/plain", "0", "cancelled\n"),
				"FUNCTION_CANCEL " + a + "\nFUNCTION_PROGRESS " + a + "\n",
				"NRPC: received a CANCEL request for call_id '" + a + "', but it is already cancelled.",
				"NRPC: received a CANCEL request for call_id '" + u + "', but the call_id is not running.",
				"Extending function timeout due to PROGRESS update...",
			},
		}
	}

	// progress (fn.call.progress, R61-2): call A (2 s); at 1.5 s the parent's FUNCTION_PROGRESS extends A to now + 10 s
	// and reaches the plugin, whose `FUNCTION_PROGRESS A 5 10` goes up as `FUNCTION_PROGRESS '<A>' 5 10` (with
	// PROGRESS); at 4.5 s call B: the GC runs (the plugin's smallest deadline is stale) and finds A alive, so no
	// CANCEL; the plugin answers B, then A. A progress for a call nobody made, from the parent and from the plugin.
	// `progress-off`: the parent refuses PROGRESS: the plugin still gets the request (pluginsd_functions.c:469-475),
	// nothing goes up (stream-sender-execute.c:96-100)
	for _, off := range []bool{false, true} {
		base := 0x600
		if off {
			base = 0x680
		}
		a, b, u := fnTx(base+1), fnTx(base+2), fnTx(base+0x7f)
		c := fnCase{
			debug: true,
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				emit(fnRegister), expect("a"), plugin.ExpectProgress("p"),
				emit(plugin.Progress("{{a}}", 5, 10) + plugin.Progress(u, 1, 2)),
				expect("b"), emit(plugin.Result("{{b}}", "200", "text/plain", "0", "b\n")),
				emit(plugin.Result("{{a}}", "200", "text/plain", "0", "a\n")),
			}}}},
			play: func(t *testing.T, x *fnSide) []string {
				t0 := time.Now()
				x.send(t, fnCall(a, 2, "difftest-fn a", "0x13", "src"))
				x.step(t, x.l, "the plugin did not get A", fnMatched(1, "a"))
				fnAt(t0, 1500*time.Millisecond)
				x.send(t, "FUNCTION_PROGRESS "+a, "FUNCTION_PROGRESS "+u)
				x.step(t, x.l, "the plugin did not get the progress request", fnMatched(1, "p"))
				fnAt(t0, 4500*time.Millisecond)
				x.send(t, fnCall(b, 2, "difftest-fn b", "0x13", "src"))
				x.answered(t, b, 1)
				x.answered(t, a, 1)
				return nil
			},
			want: []string{
				fnAnswer(a, "200", "text/plain", "0", "a\n"),
				"FUNCTION_PROGRESS " + a + "\nFUNCTION " + b,
				"Extending function timeout due to PROGRESS update...",
				"NRPC: received a PROGRESS request for call_id '" + u + "', but the call_id is not running.",
				"got a FUNCTION_PROGRESS for transaction '" + u + "', but the transaction is not found.",
			},
			wantNot: []string{"FUNCTION_CANCEL " + a, a + ": RESULT 504 "},
		}
		name := "progress"
		if off {
			name = "progress-off"
			c.refused = stream.CapProgress
			c.wantNot = append(c.wantNot, "PROGRESS 5 10")
		} else {
			c.want = append(c.want, a+": PROGRESS 5 10")
		}
		cases[name] = c
	}

	// the GC (pd.call.gc): A (2 s) unanswered; at 4.5 s B: its line first, then FUNCTION_CANCEL A (correction 18,
	// pluginsd_functions.c:433 then :494-495) and A's 504 during B's dispatch; the plugin answers B, then A late:
	// "transaction is not found", its body dropped
	{
		a, b := fnTx(0x701), fnTx(0x702)
		cases["gc"] = fnCase{
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				emit(fnRegister), expect("a"), expect("b"), plugin.ExpectCancel("x"),
				emit(plugin.Result("{{b}}", "200", "text/plain", "0", "b\n")), {SleepMs: 200},
				emit(plugin.Result("{{a}}", "200", "text/plain", "0", "late\n")),
			}}}},
			play: func(t *testing.T, x *fnSide) []string {
				t0 := time.Now()
				x.send(t, fnCall(a, 2, "difftest-fn a", "0x13", "src"))
				fnAt(t0, 4500*time.Millisecond)
				x.send(t, fnCall(b, 2, "difftest-fn b", "0x13", "src"))
				x.answered(t, a, 1)
				x.answered(t, b, 1)
				x.step(t, x.l, "the plugin did not answer late", func(s [][]plugin.Record) bool {
					return len(s) >= 1 && plugin.Has(s[0], "step", "sleep")
				})
				time.Sleep(time.Second)
				return nil
			},
			want: []string{
				fnError(a, 504, "Timeout waiting for a response."),
				fnAnswer(b, "200", "text/plain", "0", "b\n"),
				"FUNCTION " + b + " 2 \"difftest-fn b\" \"0x13\" \"src\"\nFUNCTION_CANCEL " + a + "\n",
				"got a FUNCTION_RESULT_BEGIN for transaction '" + a + "', but the transaction is not found.",
			},
		}
	}

	// no later call (R61-8): A (2 s) and B (30 s) unanswered; 4.5 s after A nothing went up and the plugin got no
	// CANCEL (the GC runs only from a dispatch); then the plugin exits: both 503 "...exited before responding."
	// (pluginsd_functions.c:104-105, the run end's sweep; also exit-before); the next start hangs
	{
		a, b := fnTx(0x801), fnTx(0x802)
		cases["nogc"] = fnCase{
			starts: 2,
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{emit(fnRegister), expect("a"), expect("b"), {WaitFile: "exit"}, {Exit: plugin.ExitCode(0)}}},
				{Steps: []plugin.Step{hang}},
			}},
			play: func(t *testing.T, x *fnSide) []string {
				t0 := time.Now()
				x.send(t, fnCall(a, 2, "difftest-fn a", "0x13", "src"), fnCall(b, 30, "difftest-fn b", "0x13", "src"))
				x.step(t, x.l, "the plugin did not get B", fnMatched(1, "b"))
				fnAt(t0, 4500*time.Millisecond)
				obs := fmt.Sprintf("4.5 s after A: answers A %d B %d, cancels on stdin %d", fnCountAnswers(x.s.Data(), a),
					fnCountAnswers(x.s.Data(), b), strings.Count(x.stdin(x.l, 1), "FUNCTION_CANCEL"))
				x.release(t, x.l, "exit")
				x.answered(t, a, 1)
				x.answered(t, b, 1)
				x.step(t, x.l, "no second start", startHangs(2))
				return []string{obs}
			},
			want: []string{
				"4.5 s after A: answers A 0 B 0, cancels on stdin 0",
				fnError(a, 503, exited), fnError(b, 503, exited),
			},
		}
	}

	// a span the plugin's exit cuts (pd.parser.teardown, R61-4): A's BEGIN 200 and one line, then the exit: 503 with
	// the partial body, its content type and expiry kept (pluginsd_functions.c:160-163); C, pending, gets
	// "...exited before responding."
	{
		a, c := fnTx(0x901), fnTx(0x902)
		cases["exit-mid"] = fnCase{
			starts: 2,
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{emit(fnRegister), expect("a"), expect("c"),
					emit("FUNCTION_RESULT_BEGIN {{a}} 200 application/json " + fnFuture + "\n{\"partial\":\n"), {SleepMs: 300},
					{Exit: plugin.ExitCode(0)}}},
				{Steps: []plugin.Step{hang}},
			}},
			play: func(t *testing.T, x *fnSide) []string {
				x.send(t, fnCall(a, 10, "difftest-fn a", "0x13", "src"), fnCall(c, 10, "difftest-fn c", "0x13", "src"))
				x.answered(t, a, 1)
				x.answered(t, c, 1)
				x.step(t, x.l, "no second start", startHangs(2))
				return nil
			},
			want: []string{fnAnswer(a, "503", "application/json", fnFuture, "{\"partial\":\n"), fnError(c, 503, exited)},
		}
	}

	// a method whose plugin exited (pd.orch.exit.functions, fn.registry.serving_thread): 503 "...not currently
	// running."; its next start cannot delete it, nor can another plugin (pd.kw.function_del.serving: the WARNING,
	// nrpc-registry.c:680-692); once difftestb registers `difftest-fn`, a call for `difftest-fn x y` strips past the
	// unavailable `difftest-fn x` to it (fn.registry.strip_past_unavailable) and difftestb gets the whole command
	{
		g1, g2 := fnTx(0xa01), fnTx(0xa02)
		mismatch := "NRPC: refusing to unregister method 'difftest-fn x' - serving-thread mismatch " +
			"(registered by another serving thread, unregister requested by current serving thread)"
		cases["gone"] = fnCase{
			starts: 2,
			enable: []string{"difftestb"},
			listed: "difftest-fn x",
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{emit(`FUNCTION GLOBAL "difftest-fn x" 10 "gone" "top" "0x13" 100 1` + "\n"),
					{WaitFile: "r1"}, {Exit: plugin.ExitCode(0)}}},
				{Steps: []plugin.Step{emit(`FUNCTION_DEL GLOBAL "difftest-fn x"` + "\n"), hang}},
			}},
			more: []morePlugin{{dir: "plugins.d", file: "difftestb.plugin", sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				{WaitFile: "r2"}, emit(fnRegister + `FUNCTION_DEL GLOBAL "difftest-fn x"` + "\n"), expect("s"),
				emit(plugin.Result("{{s}}", "200", "text/plain", "0", "b\n")),
			}}}}}},
			play: func(t *testing.T, x *fnSide) []string {
				b := x.more["difftestb.plugin"]
				x.release(t, x.l, "r1")
				x.step(t, x.l, "no second start", startHangs(2))
				time.Sleep(500 * time.Millisecond)
				x.send(t, fnCall(g1, 2, "difftest-fn x y", "0x13", "src"))
				x.answered(t, g1, 1)
				x.release(t, b, "r2")
				if x.listed(t, "difftest-fn") {
					x.send(t, fnCall(g2, 2, "difftest-fn x y", "0x13", "src"))
					x.answered(t, g2, 1)
				}
				return nil
			},
			want: []string{
				fnError(g1, 503, notRunning),
				fnAnswer(g2, "200", "text/plain", "0", "b\n"),
				"FUNCTION " + g2 + ` 2 "difftest-fn x y" "0x13" "src"`,
				mismatch,
			},
		}
	}
	// a vnode's method (R61-14): the plugin registers difftest-fn on localhost, defines the vnode, registers
	// difftest-vfn on it and collects into it; the vnode streams on a session of its own (its sender may sit on
	// another stream thread), through which the parent calls difftest-vfn; the answer goes up that session. The
	// vnode re-define's retirement (fn.registry.vnode_epoch) stays unit-tested: a re-define may restart the vnode's
	// stream.
	{
		v := fnTx(0xb01)
		vnodeReady := "STREAM SND '" + vnodeName + "': streaming is ready, sending metrics to parent..."
		cases["vnode"] = fnCase{
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				emit(fnRegister + vnodeDefine(vnodeGUID, vnodeName, vnodeLabels...) +
					`FUNCTION GLOBAL "difftest-vfn" 10 "vnode fn" "top" "0x13" 100 1` + "\n"),
				{Collect: &plugin.Collect{Chart: "difftest.vn", Dims: []string{"x"}, N: 300, Background: true}},
				expect("v"), emit(plugin.Result("{{v}}", "200", "text/plain", "0", "vnode\n")),
			}}}},
			play: func(t *testing.T, x *fnSide) []string {
				vs := x.parent.WaitSessionFor(vnodeGUID, 1, 60*time.Second)
				if vs == nil {
					return []string{"no STREAM session of the vnode within 60 s"}
				}
				x.s = vs
				if x.listed(t, "difftest-vfn") {
					x.send(t, fnCall(v, 2, "difftest-vfn", "0x13", "src"))
					x.answered(t, v, 1)
				}
				return nil
			},
			// the vnode's first block after its sender is ready logs so (command-begin-set-end-init.c:42-46), up to a
			// second after the call's answer: the stop must not cut it (run 15: present in one C vs C run, absent
			// from both sides of the other)
			settle: func(t *testing.T, x *fnSide) {
				if !pollUntil(fnWait, func() bool { return logContains(t, x.d, vnodeReady) }) {
					t.Errorf("%s: no %q within %v", x.role, vnodeReady, fnWait)
				}
			},
			want: []string{fnAnswer(v, "200", "text/plain", "0", "vnode\n"), "FUNCTION " + v + ` 2 "difftest-vfn" "0x13" "src"`,
				vnodeReady},
		}
	}
	cases["bearer"] = fnBearerCase()
	return cases
}
