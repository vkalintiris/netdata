// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"regexp"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/plugin"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// `fn.stream-proxy` (M8 commit 7, D164 B6; plan §5.4): a scripted grandparent calls a C grandchild's methods through a
// middle parent (C or candidate) that proxies the grandchild and its vnode. The middle runs each call it gets as a
// no-wait call against the grandchild's method (stream-sender-execute.c:59-104), which goes down the grandchild's
// receiver; answers and progress come back up both hops.

// fnProxyMiddleExtra proxies the grandchild and its vnode upstream ([stream]'s destination and key).
var fnProxyMiddleExtra = fmt.Sprintf("\n[%s]\n    proxy enabled = yes\n\n[%s]\n    proxy enabled = yes\n", rchildGUID, rvGUID)

// fnProxySide is one middle's side: the scripted grandparent, the middle, the C grandchild with its fake plugin, and
// the grandparent's sessions of the proxied grandchild and of its vnode.
type fnProxySide struct {
	*fnSide                // d the middle, parent the grandparent, s the grandchild's session, l its difftest
	gc      *daemon.Daemon // the grandchild
	vs      *stream.Session
}

// fnProxyStart starts one side on the test goroutine: the grandparent (plaintext, PROGRESS refused for the vnode's
// session only), the middle (`senderChild`: alloc, its own key's section, the two proxy sections, debug level) and the
// grandchild (rvChild's: the fake plugin, PULSE on) streaming to the middle with its key.
func fnProxyStart(t *testing.T, bin, cbin string, role Role, sc plugin.Scenario) *fnProxySide {
	gp, err := stream.StartParent(func(r stream.Request) stream.Answer {
		a := stream.PlaintextAnswer(r)
		caps := r.Caps() &^ stream.CapsCompression
		if r.Params.Get("machine_guid") == rvGUID {
			caps &^= stream.CapProgress
		}
		a.Reply = stream.VCaps(caps)
		return a
	})
	if err != nil {
		t.Fatalf("%s: %v", role, err)
	}
	t.Cleanup(func() { gp.Close() })
	logs := "    level = debug\n"
	x := &fnProxySide{fnSide: &fnSide{role: role, begin: time.Now(), parent: gp, more: map[string]plugin.Layout{}}}
	x.d = senderChild(t, bin, Role("fsp-m-"+string(role)), gp, "", func(o *daemon.Options) {
		o.NoStreamKey, o.StreamMemoryMode, o.LogsExtra, o.StreamExtra = false, "ram", logs, fnProxyMiddleExtra
	})
	x.gc, x.l, err = rvChild(t, cbin, Role("fsp-gc-"+string(role)), &daemon.StreamTo{Destination: x.d.Addr,
		APIKey: x.d.StreamKey, Extra: "    reconnect delay = 5\n"}, logs, sc)
	if err != nil {
		t.Fatalf("%s: the grandchild: %v", role, err)
	}
	return x
}

// fnProxyRelistRe are a session's re-list lines.
var fnProxyRelistRe = regexp.MustCompile(`^FUNCTION(?:_DEL)? GLOBAL `)

// fnProxyConfigRe is a re-list's synthetic `config` line (dyncfg.c:521-528, appended by a C sender when DYNCFG is
// common and the host has DynCfg methods, command-function.c:38-39): a DEVIATION mask until M8 commit 8 (DynCfg), not
// C's variation. The grandchild's `config` reaches the middle; C's middle re-lists it for the proxied host.
var fnProxyConfigRe = regexp.MustCompile(`^FUNCTION GLOBAL config `)

// fnProxyRelists are a session's re-lists before stop, in order: each run of `FUNCTION GLOBAL` and `FUNCTION_DEL
// GLOBAL` lines (one commit) joined, the `config` line left out (fnProxyConfigRe).
func fnProxyRelists(s *stream.Session, stop time.Time) []string {
	var out, run []string
	end := func() {
		if len(run) > 0 {
			out = append(out, strings.Join(run, " | "))
		}
		run = nil
	}
	for _, l := range fnLines(s.Chunks(), stop) {
		switch {
		case fnProxyConfigRe.MatchString(l.text):
		case fnProxyRelistRe.MatchString(l.text):
			run = append(run, l.text)
		default:
			end()
		}
	}
	end()
	return out
}

// TestFnStreamProxy (check `fn.stream-proxy`, M8 commit 7, D164 B6): per side a scripted grandparent, a middle parent
// (the oracle's or the candidate's) and an identical C grandchild running the fake plugin; the grandparent calls on
// the grandchild's proxied session and on its vnode's (the grandparent refuses PROGRESS there). Compared: the answers
// per transaction on each session (fnUpstream: spans whole, an error's expiry NOW+1), the re-lists on the grandchild's
// session, the grandchild plugin's starts (stdin byte for byte), the middle's and the grandchild's stream-thread call
// records, the observations.
func TestFnStreamProxy(t *testing.T) {
	bins := binaries(t)
	sc, play, want, wantNot := fnProxyScenario()
	var sides [2]*fnProxySide
	for i, role := range []Role{Oracle, Candidate} {
		sides[i] = fnProxyStart(t, bins[i], bins[0], role, sc)
	}
	var obs [2][]string
	var played [2]bool
	var wg sync.WaitGroup
	for i, x := range sides {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if x.s = x.parent.WaitSessionFor(rchildGUID, 1, 90*time.Second); x.s == nil {
				t.Errorf("%s: no proxied session of the grandchild within 90 s", x.role)
				return
			}
			if !x.listed(t, "difftest-fn") {
				return
			}
			x.release(t, x.l, "vnode")
			if x.vs = x.parent.WaitSessionFor(rvGUID, 1, 60*time.Second); x.vs == nil {
				t.Errorf("%s: no proxied session of the vnode within 60 s", x.role)
				return
			}
			vs := &fnSide{role: x.role, s: x.vs}
			if !vs.listed(t, "difftest-x") {
				return
			}
			obs[i], played[i] = play(t, x), true
		}()
	}
	wg.Wait()
	if !played[0] {
		t.Fatal("oracle: the cases did not play")
	}
	type result struct {
		answers, vanswers, relists, vrelists, views, stdin, middle, gc []string
		configs                                                        int
	}
	var res [2]result
	for i, x := range sides {
		stop := time.Now()
		if err := x.d.Stop(); err != nil {
			t.Errorf("%s: stop the middle: %v", x.role, err)
		}
		lo, hi := x.begin.Unix()-5, stop.Unix()+5
		r := &res[i]
		if x.s != nil {
			r.answers = fnUpstream(fnLines(x.s.Chunks(), stop), lo, hi)
			r.relists = fnProxyRelists(x.s, stop)
			for _, l := range fnLines(x.s.Chunks(), stop) {
				if fnProxyConfigRe.MatchString(l.text) {
					r.configs++
				}
			}
		}
		if x.vs != nil {
			r.vanswers = fnUpstream(fnLines(x.vs.Chunks(), stop), lo, hi)
			r.vrelists = fnProxyRelists(x.vs, stop)
		}
		starts, err := x.l.Starts()
		if err != nil {
			t.Errorf("%s: %v", x.role, err)
		}
		r.views, _ = fnViews(plugin.Name, starts, stop)
		for _, s := range starts {
			r.stdin = append(r.stdin, plugin.ViewOf(plugin.Before(s, stop)).Stdin)
		}
		r.middle = fnStreamThreadRecords(t, x.d, stop)
		r.gc = fnStreamThreadRecords(t, x.gc, stop)
	}
	o := res[0]
	if len(o.views) == 0 {
		t.Errorf("oracle: the grandchild's plugin never started")
	}
	hay := slices.Concat(o.answers, o.vanswers, o.relists, o.vrelists, obs[0], o.stdin, o.middle, o.gc)
	for _, w := range want {
		if !slices.ContainsFunc(hay, func(l string) bool { return strings.Contains(l, w) }) {
			t.Errorf("oracle: nothing holds %q", w)
		}
	}
	for _, w := range wantNot {
		if k := slices.IndexFunc(hay, func(l string) bool { return strings.Contains(l, w) }); k >= 0 {
			t.Errorf("oracle: %q holds %q", hay[k], w)
		}
	}
	// the deviation mask is not stale: C's middle re-lists the grandchild's `config` (its DynCfg methods)
	if o.configs == 0 {
		t.Errorf("oracle: no `config` line on the grandchild's session: fnProxyConfigRe is stale")
	}
	diffLines(t, "the grandchild's answers", o.answers, res[1].answers)
	diffLines(t, "the vnode's answers", o.vanswers, res[1].vanswers)
	diffLines(t, "the grandchild's re-lists", o.relists, res[1].relists)
	diffLines(t, "the vnode's re-lists", o.vrelists, res[1].vrelists)
	diffLines(t, "observations", obs[0], obs[1])
	diffLines(t, "the grandchild plugin's starts", o.views, res[1].views)
	diffLines(t, "the middle's stream-thread call records", o.middle, res[1].middle)
	diffLines(t, "the grandchild's stream-thread call records", o.gc, res[1].gc)
	t.Logf("oracle answers:\n%s\nvnode answers:\n%s\nre-lists:\n%s\nvnode re-lists:\n%s\nobservations:\n%s\nplugin starts:\n%s\n"+
		"middle records:\n%s\ngrandchild records:\n%s", strings.Join(o.answers, "\n"), strings.Join(o.vanswers, "\n"),
		strings.Join(o.relists, "\n"), strings.Join(o.vrelists, "\n"),
		strings.Join(obs[0], "\n"), strings.Join(o.views, "\n"), strings.Join(o.middle, "\n"), strings.Join(o.gc, "\n"))
}

// fnProxyScenario is the grandchild's plugin, the grandparent's play (in order, on each side) and the oracle's guards.
func fnProxyScenario() (plugin.Scenario, func(t *testing.T, x *fnProxySide) []string, []string, []string) {
	tx := func(n int) string { return fnTx(0x5000 + n) }
	emit := func(s string) plugin.Step { return plugin.Step{Emit: s} }
	expect := plugin.ExpectFunction
	ok := func(name string) plugin.Step {
		return emit(plugin.Result("{{"+name+"}}", "200", "text/plain", "0", name+"\n"))
	}
	xRegister := `FUNCTION GLOBAL "difftest-x" 10 "to delete" "top" "0x13" 100 1` + "\n"
	vfnRegister := `FUNCTION GLOBAL "difftest-vfn" 10 "vnode fn" "top" "0x13" 100 1` + "\n"
	codes := []string{
		plugin.Result("{{c}}", "404", "application/json", fnFuture, "{\"status\":404,\"error_message\":\"plugin says no\"}\n"),
		"FUNCTION_RESULT_BEGIN {{c}} 0 text/plain 0\nzero\nFUNCTION_RESULT_END\n",
		plugin.Result("{{c}}", "200", "text/plain", "1", "past\n"),
	}
	steps := []plugin.Step{
		emit(fnRegister + fnHiddenRegister + vnodeDefine(rvGUID, rvName, rvLabels...) + vfnRegister + xRegister),
		{WaitFile: "vnode"}, {Collect: &plugin.Collect{Chart: rvChart, Dims: []string{"x"}, N: 900, Background: true}},
		expect("a"), emit(plugin.Result("{{a}}", "200", "application/json", fnFuture, "{\"ok\":1}\n")),
		plugin.ExpectPayload("p"), ok("p"),
	}
	for _, c := range codes {
		steps = append(steps, expect("c"), emit(c))
	}
	steps = append(steps,
		expect("r"), ok("r"),
		expect("k"), plugin.ExpectCancel("kx"), ok("k"),
		expect("q"), emit(plugin.Progress("{{q}}", 5, 10)), plugin.ExpectProgress("qp"), plugin.Step{WaitFile: "q"}, ok("q"),
		expect("v"), emit(plugin.Progress("{{v}}", 5, 10)), plugin.ExpectProgress("vp"), plugin.Step{WaitFile: "v"}, ok("v"),
		expect("g"), expect("h"), plugin.ExpectCancel("gx"), ok("h"),
		plugin.Step{WaitFile: "del"}, emit(`FUNCTION_DEL GLOBAL "difftest-x"`+"\n"),
		expect("d"), plugin.Step{Hang: true},
	)
	a, p, r, k, q, v, g, h, n, d := tx(1), tx(2), tx(6), tx(7), tx(8), tx(9), tx(10), tx(11), tx(12), tx(13)
	c := func(i int) string { return tx(3 + i) }
	play := func(t *testing.T, x *fnProxySide) []string {
		var out []string
		// a call and its answer through both hops: a `\n` more per hop (P1)
		x.send(t, fnCall(a, 2, "difftest-fn a", "0x13", "src"))
		x.answered(t, a, 1)
		// a payload through both hops: each adds a line before the end
		x.send(t, `FUNCTION_PAYLOAD `+p+` 2 "difftest-fn p" "0x13" "src" "application/json"`, `{"a":1}`, `FUNCTION_PAYLOAD_END`)
		x.answered(t, p, 1)
		// the plugin's 404 and status 0 (591) and a past expiry (0) through both hops
		for i := range codes {
			x.send(t, fnCall(c(i), 2, fmt.Sprintf("difftest-fn c%d", i), "0x13", "src"))
			x.answered(t, c(i), 1)
		}
		// a restricted method: each hop's call is allowed it (allow_restricted, stream-sender-execute.c:85)
		x.send(t, fnCall(r, 2, "__difftest-hidden", "0x0", "src"))
		x.answered(t, r, 1)
		// the grandparent's cancel reaches the plugin; the plugin's late answer comes up
		x.send(t, fnCall(k, 10, "difftest-fn k", "0x13", "src"))
		x.step(t, x.l, "the plugin did not get K", fnMatched(1, "k"))
		x.send(t, "FUNCTION_CANCEL "+k)
		out = append(out, fmt.Sprintf("the plugin got K's cancel: %t", x.step(t, x.l, "no cancel", fnMatched(1, "kx"))))
		x.answered(t, k, 1)
		// progress: the plugin's 5 of 10 comes up (the grandparent's link has PROGRESS); the grandparent's progress
		// request at 1.5 s extends Q at each hop and reaches the plugin; Q's answer at 3.5 s, past its 2 s, is a 200
		t0 := time.Now()
		x.send(t, fnCall(q, 2, "difftest-fn q", "0x13", "src"))
		x.step(t, x.l, "the plugin did not get Q", fnMatched(1, "q"))
		fnAt(t0, 1500*time.Millisecond)
		x.send(t, "FUNCTION_PROGRESS "+q)
		out = append(out, fmt.Sprintf("the plugin got Q's progress request: %t", x.step(t, x.l, "no progress request", fnMatched(1, "qp"))))
		fnAt(t0, 3500*time.Millisecond)
		x.release(t, x.l, "q")
		x.answered(t, q, 1)
		// the same on the vnode's session, whose link refused PROGRESS: nothing comes up of the plugin's 5 of 10; the
		// progress request still goes down the middle's link to the grandchild
		vs := &fnSide{role: x.role, s: x.vs}
		t0 = time.Now()
		vs.send(t, fnCall(v, 2, "difftest-vfn v", "0x13", "src"))
		x.step(t, x.l, "the plugin did not get V", fnMatched(1, "v"))
		fnAt(t0, 1500*time.Millisecond)
		vs.send(t, "FUNCTION_PROGRESS "+v)
		out = append(out, fmt.Sprintf("the plugin got V's progress request: %t", x.step(t, x.l, "no progress request", fnMatched(1, "vp"))))
		fnAt(t0, 3500*time.Millisecond)
		x.release(t, x.l, "v")
		vs.answered(t, v, 1)
		// the middle's GC: G (1 s) unanswered; H at 3 s: the middle's dispatch of H answers G 504 up and cancels it
		// down after H's line; the grandchild's own GC cancels G to the plugin after H's line
		t0 = time.Now()
		x.send(t, fnCall(g, 1, "difftest-fn g", "0x13", "src"))
		fnAt(t0, 3*time.Second)
		x.send(t, fnCall(h, 2, "difftest-fn h", "0x13", "src"))
		x.answered(t, g, 1)
		x.answered(t, h, 1)
		// an unknown method: the middle's 404
		x.send(t, fnCall(n, 2, "nope", "0x13", "src"))
		x.answered(t, n, 1)
		// the plugin deletes the vnode's method (its scope is the vnode): the grandchild's FUNCTION_DEL reaches the
		// middle, whose re-list for the proxied vnode carries it up (D104.6, D106.7)
		x.release(t, x.l, "del")
		del := "\nFUNCTION_DEL GLOBAL \"difftest-x\"\n"
		out = append(out, fmt.Sprintf("the grandparent got the delete: %t",
			x.vs.WaitData(func(b []byte) bool { return strings.Contains(string(b), del) }, 30*time.Second)))
		// the grandchild killed while its plugin holds D: the middle's sweep answers D to a no-wait call whose host is
		// no longer online, so nothing comes up (stream-sender-execute.c:17); the proxied session ends
		x.send(t, fnCall(d, 10, "difftest-fn d", "0x13", "src"))
		if x.step(t, x.l, "the plugin did not get D", fnMatched(1, "d")) {
			if err := x.gc.Kill(); err != nil {
				t.Errorf("%s: kill the grandchild: %v", x.role, err)
			}
		}
		closed := pollUntil(30*time.Second, x.s.Closed)
		return append(out, fmt.Sprintf("the proxied session closed: %t", closed),
			fmt.Sprintf("answers for D: %d", fnCountAnswers(x.s.Data(), d)))
	}
	want := []string{
		fnAnswer(a, "200", "application/json", fnFuture, "{\"ok\":1}\n\n"),
		"FUNCTION_PAYLOAD " + p + ` 2 "difftest-fn p" "0x13" "src" "application/json"` + "\n{\"a\":1}\n\n\nFUNCTION_PAYLOAD_END\n",
		// a future expiry is kept up both hops whatever the code (pluginsd_functions.c:697-701)
		fnAnswer(c(0), "404", "application/json", fnFuture, "{\"status\":404,\"error_message\":\"plugin says no\"}\n\n"),
		c(1) + `: RESULT 591 "text/plain"`,
		fnAnswer(c(2), "200", "text/plain", "0", "past\n\n"),
		"FUNCTION " + r + ` 2 "__difftest-hidden" "0x0" "src"`,
		"the plugin got K's cancel: true", "FUNCTION_CANCEL " + k + "\n",
		k + ": RESULT 200",
		q + ": PROGRESS 5 10", "the plugin got Q's progress request: true", q + ": RESULT 200",
		"the plugin got V's progress request: true", v + ": RESULT 200",
		fnError(g, 504, "Timeout waiting for a response."),
		"FUNCTION " + h + ` 2 "difftest-fn h" "0x13" "src"` + "\nFUNCTION_CANCEL " + g + "\n",
		fnError(n, 404, "This feature is not available on this host at this time."),
		"the grandparent got the delete: true", `FUNCTION_DEL GLOBAL "difftest-x" | FUNCTION GLOBAL "difftest-vfn" `,
		"the proxied session closed: true", "answers for D: 0",
	}
	wantNot := []string{v + ": PROGRESS", g + ": RESULT 200"}
	return fnScenario(steps...), play, want, wantNot
}
