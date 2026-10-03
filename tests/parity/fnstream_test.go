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
)

// Functions over streaming (M8 commit 7, D164; plan evidence/2026-10-02-plan-m8-commit7.md §5): a parent runs a call
// for a child's method through the child's receiver (`send_to_plugin_cb = send_to_child`, stream-receiver.c:647-651)
// and relays the answer. `fn.stream` runs a C parent and a candidate parent, each over an identical C child running
// the fake plugin, in three shared topologies (B7).

// fnStreamTx is a transaction an fn.stream case sends: fnTx's scheme, so fnMaskIDs keeps it.
func fnStreamTx(n int) string { return fnTx(0x3000 + n) }

// fnChildHost is the parents' route to the child's host.
const fnChildHost = "/host/" + rchildHostname

// fnStreamSide is one parent's side of an fn.stream topology: the parent (fnHTTPSide's d; its l and more are the C
// child's fake plugins) and that child.
type fnStreamSide struct {
	*fnHTTPSide
	child *daemon.Daemon
}

// fnStreamCase is one scenario of a topology: its play on each side (t.Errorf only), the oracle's guards.
type fnStreamCase struct {
	name string
	play func(t *testing.T, x *fnStreamSide) []string
	// want are texts the oracle's exchanges of this case, the child plugin's stdin, the parents' access and call
	// records, the child's call records or its plugin log classes must hold (each by substring); wantNot are texts
	// none of them may hold
	want, wantNot []string
}

// fnStreamTopo is one topology of `fn.stream`: what the C children run, what each side waits for, the cases played
// in order on both sides at once.
type fnStreamTopo struct {
	// sc is the child's difftest scenario (every case's steps in order); more are the child's other fake plugins,
	// enabled by enable
	sc     plugin.Scenario
	more   []morePlugin
	enable []string
	// debug runs the parents and the children at `[logs] level = debug` (nRPC's cancel and progress records)
	debug bool
	// ready, when set, runs on each side (t.Errorf only) once the parent has the child online, before the listing
	// wait; false stops the side
	ready func(t *testing.T, x *fnStreamSide) bool
	// listed are the methods each parent must list before the cases play
	listed []fnListed
	cases  []fnStreamCase
	// compare, when set, runs once both sides played, before the children stop (a comparison of the parents' state)
	compare func(t *testing.T, p *Pair, sides [2]*fnStreamSide)
	// starts is how many starts of difftest the oracle's child must show (1 when 0)
	starts int
}

// fnStreamChild starts one side's C child: `stream.rchild`'s identity (rvChild: the fake plugin as the plugin checks
// run it, PULSE on, alloc, one tier), streaming to the side's parent with the parents' key; its other fake plugins
// installed and enabled too.
func fnStreamChild(t *testing.T, bin string, role Role, parent, logs string, topo fnStreamTopo) (*daemon.Daemon,
	plugin.Layout, map[string]plugin.Layout, error) {
	engine, err := plugin.Engine()
	if err != nil {
		return nil, plugin.Layout{}, nil, err
	}
	more := map[string]plugin.Layout{}
	var installErr error
	d, l, err := rvChild(t, bin, role, rvTo(parent), logs, topo.sc, func(o *daemon.Options) {
		o.PluginsExtra = pluginsOptions(1, nil, topo.enable, logs).PluginsExtra
		for _, m := range topo.more {
			ml, err := plugin.InstallAs(o.RunDir, filepath.Join(o.RunDir, m.dir), m.file, engine, m.sc)
			if err != nil {
				installErr = err
			}
			more[m.file] = ml
		}
	})
	if err == nil {
		err = installErr
	}
	return d, l, more, err
}

// fnStreamResult is what one side left once its child stopped: the child plugins' views and stdin, the child's
// plugin log classes and call records, the parent's access records and call records per thread class.
type fnStreamResult struct {
	starts int
	// views are fnViews' lines; stdin each start's whole stdin, unmasked (the guards')
	views, stdin  []string
	classes       map[string][]string
	childRecords  []string
	access        []string
	parentRecords map[string][]string
}

// fnStreamParentRecords are a parent's call records (fnStreamRecordRe) its web workers and stream threads wrote before
// stop, per thread (WEB[n], STREAM[n]: the number masked), each in file order, normalized and masked as fn.child's;
// C's variation masked too: the connection id (fnStreamConnRe), a wall-clock expiry in the RESULT_BEGIN a record
// quotes (an error's is the child's second + 1: the clock). The records of a child's STREAM request (`request="key=…`)
// are left out: which of a child and its vnode connects first is random (R64-1c).
func fnStreamParentRecords(t *testing.T, d *daemon.Daemon, stop time.Time) map[string][]string {
	t.Helper()
	out := map[string][]string{}
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		th := threadOf(l)
		if !strings.HasPrefix(th, "WEB[") && !strings.HasPrefix(th, "STREAM[") || !fnStreamRecordRe.MatchString(l) ||
			!recordBefore(l, stop) || strings.Contains(l, ` request="key=`) {
			continue
		}
		n := fnStreamConnRe.ReplaceAllString(fnMaskRecord(normalizeLog(l, d.Opts.RunDir, "")), " conn=N ")
		n = fnStreamExpiresRe.ReplaceAllString(n, "${1}T'")
		class := "thread=" + threadOf(n)
		out[class] = append(out[class], n)
	}
	return out
}

// fnStreamThreadRecords are an agent's stream-thread call records before stop (fnStreamRecords), a quoted
// RESULT_BEGIN's wall-clock expiry masked (fnStreamParentRecords').
func fnStreamThreadRecords(t *testing.T, d *daemon.Daemon, stop time.Time) []string {
	t.Helper()
	var out []string
	for _, l := range fnStreamRecords(t, d, stop) {
		out = append(out, fnStreamExpiresRe.ReplaceAllString(l, "${1}T'"))
	}
	return out
}

// fnStreamExpiresRe is the expiry of a FUNCTION_RESULT_BEGIN a record quotes, when it is a wall-clock second.
var fnStreamExpiresRe = regexp.MustCompile(`('FUNCTION_RESULT_BEGIN' '[^']*' '[^']*' '[^']*' ')\d{10}'`)

// fnStreamAccess are a parent's request records written before `before`, in file order, normalized (fnHTTPAccess's
// form: a transaction a case sent kept as `sent:<tx>`), the connection id masked (fnStreamConnRe). Left out: the
// probes and harness=wait polls themselves (fnHTTPProbeRe; by their request, not by their client port as
// fnHTTPAccess does: these checks poll often enough for a later request to reuse a poll's port, H10's final2 lost one
// so); the children's STREAM requests and their connectors' stream_info probes (`stream.handshake`'s; how many probes
// a connector makes, and which of a child and its vnode connects first, are each run's timing: R64-1c, H10's probe2);
// the debug level's connection records (CONNECTED, DISCONNECTED: no request; C writes a stale errno on some, H10's
// wire1 `errno="11, Resource temporarily unavailable"` on one side's DISCONNECTED only).
func fnStreamAccess(t *testing.T, d *daemon.Daemon, before time.Time) []string {
	t.Helper()
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "access.log") {
		if !strings.Contains(l, ` request=`) || fnHTTPProbeRe.MatchString(l) || strings.Contains(l, ` request="key=`) ||
			strings.Contains(l, ` request="/api/v3/stream_info?`) || !loggedBefore(l, before) {
			continue
		}
		l = strings.ReplaceAll(l, " transaction="+fnTxPrefix, " transaction=sent:"+fnTxPrefix)
		out = append(out, fnStreamConnRe.ReplaceAllString(normalizeLog(l, d.Opts.RunDir, ""), " conn=N "))
	}
	return out
}

// fnStreamConnRe is a web record's connection id: C's web client cache numbers a client object only when it allocates
// one (web_client_cache.c:104-108), so which object serves a request, and its id, follow how many connections were
// open at once before (the children's STREAM requests, the harness's polls): H10's final2 had the admin's call at
// conn=2 and the duplicate's at 3 on one C parent, 0 and 2 on the other.
var fnStreamConnRe = regexp.MustCompile(` conn=\d+ `)

// fnStreamInfo is a parent's stream_info of a host (tagged harness=wait: how many polls there are is each side's
// timing).
func fnStreamInfo(addr, guid string) []byte {
	b, _ := rawExchange(addr, []byte("GET /api/v3/stream_info?machine_guid="+guid+"&harness=wait HTTP/1.1\r\n\r\n"),
		5*time.Second)
	return b
}

// fnStreamOnline waits up to `limit` until a parent says a host's ingestion is online (ingestOnline's poll).
func fnStreamOnline(addr, guid string, limit time.Duration) bool {
	return pollUntil(limit, func() bool {
		return bytes.Contains(fnStreamInfo(addr, guid), []byte(`"ingest_status":"online"`))
	})
}

// fnStreamHasChild waits up to `limit` until a parent has a host as a child (a receiver), or, with !child, knows it
// no longer as one.
func fnStreamHasChild(addr, guid string, child bool, limit time.Duration) bool {
	return pollUntil(limit, func() bool {
		b := fnStreamInfo(addr, guid)
		return bytes.Contains(b, []byte(`"ingest_type":"child"`)) == child && bytes.Contains(b, []byte(`"ingest_type":`))
	})
}

// fnStreamCollect stops a side's child and takes what the side left, cut at that stop (fnCollect's reasons: what
// reaches the parent once the stop began is a race); the parent's access records end when the cases did (`played`).
func fnStreamCollect(t *testing.T, x *fnStreamSide, played time.Time) fnStreamResult {
	var res fnStreamResult
	stop := time.Now()
	if err := x.child.Stop(); err != nil {
		t.Errorf("%s: stop the child: %v", x.role, err)
	}
	all, err := x.l.Starts()
	if err != nil {
		t.Errorf("%s: %v", x.role, err)
	}
	res.starts = len(all)
	res.views, _ = fnViews(plugin.Name, all, stop)
	for _, s := range all {
		res.stdin = append(res.stdin, plugin.ViewOf(plugin.Before(s, stop)).Stdin)
	}
	for _, file := range slices.Sorted(maps.Keys(x.more)) {
		more, err := x.more[file].Starts()
		if err != nil {
			t.Errorf("%s: %v", x.role, err)
		}
		v, _ := fnViews(file, more, stop)
		res.views = append(res.views, v...)
		for _, s := range more {
			res.stdin = append(res.stdin, plugin.ViewOf(plugin.Before(s, stop)).Stdin)
		}
	}
	res.classes = map[string][]string{}
	for class, lines := range pluginLogClassesBefore(t, x.child, stop) {
		if strings.Contains(class, "thread=PD[") {
			for _, l := range lines {
				res.classes[class] = append(res.classes[class], fnMaskRecord(l))
			}
		}
	}
	res.childRecords = fnStreamThreadRecords(t, x.child, stop)
	res.access = fnStreamAccess(t, x.d, played)
	res.parentRecords = fnStreamParentRecords(t, x.d, stop)
	return res
}

// runFnStream starts a topology (two parents, the oracle's and the candidate's, `ram`, one tier, PULSE off, fn.http's
// bearer tokens; under each an identical C child), waits until each parent has the child online (then the topology's
// ready hook) and lists the methods, plays the cases in order on both sides at once, then compares per case the
// exchanges and, once the children stopped, the child plugins' starts (stdin byte for byte), the children's plugin log
// classes and call records, the parents' access records and call records per thread; and checks each case's oracle
// guards.
func runFnStream(t *testing.T, topo fnStreamTopo) {
	bins := binaries(t)
	logs := ""
	if topo.debug {
		logs = "    level = debug\n"
	}
	p := startPairWith(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, LogsExtra: logs, PulseOff: true},
		parentIdentity, bins, [2]string{}, [2]Role{"fns-p-oracle", "fns-p-candidate"}, fnWriteTokens)
	var sides [2]*fnStreamSide
	for i, side := range p.Each() {
		child, l, more, err := fnStreamChild(t, bins[0], Role("fns-c-"+string(side.Role)), side.Daemon.Addr, logs, topo)
		if err != nil {
			t.Fatalf("%s: the C child: %v", side.Role, err)
		}
		sides[i] = &fnStreamSide{fnHTTPSide: &fnHTTPSide{role: side.Role, d: side.Daemon, l: l, more: more},
			child: child}
	}
	for i, x := range sides {
		if !fnStreamOnline(x.d.Addr, rchildGUID, 90*time.Second) {
			if i == 0 {
				t.Fatalf("%s: the child not online within 90 s", x.role)
			}
			t.Errorf("%s: the child not online within 90 s", x.role)
		}
	}
	var obs [2][][]string
	var played [2]bool
	var wg sync.WaitGroup
	for i, x := range sides {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if topo.ready != nil && !topo.ready(t, x) || !x.listed(t, topo.listed, "") {
				return
			}
			for _, c := range topo.cases {
				obs[i] = append(obs[i], c.play(t, x))
			}
			played[i] = true
		}()
	}
	wg.Wait()
	if !played[0] {
		t.Fatal("oracle: the cases did not play")
	}
	// the access record of a call is written once its answer went out; the comparison's requests are not the cases'
	time.Sleep(time.Second)
	played0 := time.Now()
	if topo.compare != nil {
		topo.compare(t, p, sides)
	}
	var res [2]fnStreamResult
	for i, x := range sides {
		res[i] = fnStreamCollect(t, x, played0)
	}
	o := res[0]
	if want := max(topo.starts, 1); o.starts != want {
		t.Errorf("oracle: %d starts of %s, want %d", o.starts, plugin.Name, want)
	}
	hay := slices.Concat(o.stdin, o.childRecords, o.access)
	for _, lines := range o.classes {
		hay = append(hay, lines...)
	}
	for _, lines := range o.parentRecords {
		hay = append(hay, lines...)
	}
	for k, c := range topo.cases {
		t.Run(c.name, func(t *testing.T) {
			var cand []string
			if k < len(obs[1]) {
				cand = obs[1][k]
			}
			diffLines(t, "exchanges", obs[0][k], cand)
			all := slices.Concat(obs[0][k], hay)
			for _, w := range c.want {
				if !slices.ContainsFunc(all, func(l string) bool { return strings.Contains(l, w) }) {
					t.Errorf("oracle: nothing holds %q", w)
				}
			}
			for _, w := range c.wantNot {
				if i := slices.IndexFunc(all, func(l string) bool { return strings.Contains(l, w) }); i >= 0 {
					t.Errorf("oracle: %q holds %q", all[i], w)
				}
			}
			t.Logf("oracle exchanges:\n%s", strings.Join(obs[0][k], "\n"))
		})
	}
	diffLines(t, "child plugin starts", o.views, res[1].views)
	diffLines(t, "child call records", o.childRecords, res[1].childRecords)
	for _, class := range slices.Sorted(maps.Keys(o.classes)) {
		diffLines(t, "child "+class, o.classes[class], res[1].classes[class])
	}
	for class := range res[1].classes {
		if _, ok := o.classes[class]; !ok {
			t.Errorf("candidate only: child %s: %q", class, res[1].classes[class])
		}
	}
	diffLines(t, "parent access records", o.access, res[1].access)
	for _, class := range slices.Sorted(maps.Keys(o.parentRecords)) {
		diffLines(t, "parent "+class, o.parentRecords[class], res[1].parentRecords[class])
	}
	for class := range res[1].parentRecords {
		if _, ok := o.parentRecords[class]; !ok {
			t.Errorf("candidate only: parent %s: %q", class, res[1].parentRecords[class])
		}
	}
	t.Logf("oracle child plugin starts:\n%s\nchild call records:\n%s\nparent access records:\n%s",
		strings.Join(o.views, "\n"), strings.Join(o.childRecords, "\n"), strings.Join(o.access, "\n"))
	for _, class := range slices.Sorted(maps.Keys(o.parentRecords)) {
		t.Logf("oracle parent %s:\n%s", class, strings.Join(o.parentRecords[class], "\n"))
	}
	for _, class := range slices.Sorted(maps.Keys(o.classes)) {
		t.Logf("oracle child %s:\n%s", class, strings.Join(o.classes[class], "\n"))
	}
}

// TestFnStream (check `fn.stream`, M8 commit 7, D164 B6-B7): a C parent and a candidate parent, each over an
// identical C child running the fake plugin, called over `/host/<child>/api/v1|v3/function`; three topologies:
// `calls`, `timing`, `disconnect`.
func TestFnStream(t *testing.T) {
	t.Run("calls", func(t *testing.T) { runFnStream(t, fnStreamCalls()) })
	t.Run("timing", func(t *testing.T) { runFnStream(t, fnStreamTiming()) })
	t.Run("disconnect", func(t *testing.T) { runFnStream(t, fnStreamDisconnect()) })
}

// fnStreamCalls is the `calls` topology: one call after another, each answered. The child's plugin registers on
// localhost and on the vnode it defines; difftestb registers `difftest-gone` once difftest's methods are listed (the
// re-list's order), then exits when the case says.
func fnStreamCalls() fnStreamTopo {
	tx := fnStreamTx
	emit := func(s string) plugin.Step { return plugin.Step{Emit: s} }
	expect := plugin.ExpectFunction
	ok := func(name string) plugin.Step {
		return emit(plugin.Result("{{"+name+"}}", "200", "text/plain", "0", name+"\n"))
	}
	call := func(query string) string { return fnChildHost + "/api/v1/function?" + query }
	vcall := func(query string) string { return "/host/" + rvName + "/api/v1/function?" + query }
	vfnRegister := `FUNCTION GLOBAL "difftest-vfn" 10 "vnode fn" "top" "0x8" 100 1` + "\n"
	goneRegister := `FUNCTION GLOBAL "difftest-gone" 10 "gone" "top" "0x8" 100 1` + "\n"
	topo := fnStreamTopo{
		enable: []string{"difftestb"},
		// difftestb registers once the parent lists difftest's methods (two plugins starting at once register in
		// either order); the vnode's sender starts at its first collection, released once the parent has the child,
		// so the parent creates the child's host first (its v2 node index; R64-1c)
		ready: func(t *testing.T, x *fnStreamSide) bool {
			if !x.listed(t, []fnListed{{fnChildHost, "difftest-open"}}, "") {
				return false
			}
			x.release(t, x.more["difftestb.plugin"], "register")
			x.release(t, x.l, "vnode")
			if !fnStreamOnline(x.d.Addr, rvGUID, 60*time.Second) {
				t.Errorf("%s: the vnode not online within 60 s", x.role)
				return false
			}
			return true
		},
		listed: []fnListed{{fnChildHost, "difftest-gone"}, {"/host/" + rvName, "difftest-vfn"}},
		more: []morePlugin{{dir: "plugins.d", file: "difftestb.plugin", sc: plugin.Scenario{Starts: []plugin.Start{
			{Steps: []plugin.Step{{WaitFile: "register"}, emit(goneRegister), {WaitFile: "gone"}, {Exit: plugin.ExitCode(0)}}},
			{Steps: []plugin.Step{{Hang: true}}},
		}}}},
	}
	steps := []plugin.Step{
		emit(fnOpenRegister + fnRegister + fnHiddenRegister + vnodeDefine(rvGUID, rvName, rvLabels...) + vfnRegister),
		{WaitFile: "vnode"}, {Collect: &plugin.Collect{Chart: rvChart, Dims: []string{"x"}, N: 900, Background: true}},
	}
	add := func(c fnStreamCase, s ...plugin.Step) {
		topo.cases = append(topo.cases, c)
		steps = append(steps, s...)
	}

	// a call's line on the child plugin's stdin is the parent's, rewritten by the child (the same words here); the
	// body comes up with one more `\n` than the plugin wrote (the child's FUNCTION_RESULT_END framing,
	// stream-sender-execute.c:21-26, kept by the parent's deferred read, pluginsd_parser.h:202-237: P1); the plugin's
	// constant expiry stays cacheable through the hop
	add(fnStreamCase{name: "ok",
		play: func(t *testing.T, x *fnStreamSide) []string {
			return []string{x.do(t, "v1", fnHTTPGet(call("function=difftest-open%20a&timeout=3"), tx(1)))}
		},
		want: []string{fnHTTPLine(tx(1), 3, "difftest-open a"), fnQ("HTTP/1.1 200 OK\r\n"),
			fnQ("Cache-Control: public\r\nExpires: " + fnFutureHTTP + "\r\nContent-Length: 10\r\nX-Transaction-ID: " + tx(1) +
				"\r\n\r\n{\"ok\":1}\n\n")},
	}, expect("a"), emit(plugin.Result("{{a}}", "200", "application/json", fnFuture, "{\"ok\":1}\n")))

	add(fnStreamCase{name: "v3",
		play: func(t *testing.T, x *fnStreamSide) []string {
			return []string{x.do(t, "v3", fnHTTPGet(fnChildHost+"/api/v3/function?function=difftest-open%20v3&timeout=3", tx(2)))}
		},
		want: []string{fnHTTPLine(tx(2), 3, "difftest-open v3"), fnQ("X-Transaction-ID: " + tx(2) + "\r\n\r\nb\n\n")},
	}, expect("b"), ok("b"))

	// payloads through the hop: the parent's FUNCTION_PAYLOAD block (pluginsd_functions.c:24-38), which the child's
	// deferred read keeps with each line's newline, and writes to its plugin with one more line before the end
	add(fnStreamCase{name: "payload",
		play: func(t *testing.T, x *fnStreamSide) []string {
			body := func(method, cmd, txn string, headers []string, b string) []byte {
				return rawRequest(method, call("function=difftest-open%20"+cmd), append([]string{"X-Transaction-Id: " + txn}, headers...), []byte(b))
			}
			return []string{
				x.do(t, "post json", body("POST", "j", tx(11), []string{"Content-Type: application/json"}, "{\"a\":1}\n{\"b\":2}")),
				x.do(t, "put", body("PUT", "u", tx(12), []string{"Content-Type: text/plain; charset=utf-8"}, "x=1\n")),
				x.do(t, "post unknown", body("POST", "n", tx(13), []string{"Content-Type: application/x-nope"}, "n")),
				x.do(t, "post empty", body("POST", "e", tx(14), []string{"Content-Type: application/json"}, "")),
			}
		},
		want: []string{
			"FUNCTION_PAYLOAD " + tx(11) + ` 10 "difftest-open j" "0x8" "method=none,role=any,permissions=0x8,ip=localhost" "application/json"` +
				"\n{\"a\":1}\n{\"b\":2}\n\nFUNCTION_PAYLOAD_END\n",
			"FUNCTION_PAYLOAD " + tx(12) + ` 10 "difftest-open u" "0x8" "method=none,role=any,permissions=0x8,ip=localhost" "text/plain"` +
				"\nx=1\n\n\nFUNCTION_PAYLOAD_END\n",
			"FUNCTION_PAYLOAD " + tx(13) + ` 10 "difftest-open n" "0x8" "method=none,role=any,permissions=0x8,ip=localhost" "text/plain"` +
				"\nn\n\nFUNCTION_PAYLOAD_END\n",
			fnHTTPLine(tx(14), 10, "difftest-open e"),
		},
	}, plugin.ExpectPayload("p1"), ok("p1"), plugin.ExpectPayload("p2"), ok("p2"), plugin.ExpectPayload("p3"), ok("p3"),
		expect("p4"), ok("p4"))

	// what the plugin's FUNCTION_RESULT_BEGIN words become through one hop: the child's parser maps them
	// (fn.child `codes`), the parent's HTTP answer as fn.http's `expires`, except the quoted empty type: the child
	// sends text/plain up (P2: a relayed answer's cacheability is the parent's rule, any non-200 no-cache)
	{
		answers := []string{
			plugin.Result("{{c}}", "200", "application/json", "1", "{\"past\":1}\n"),
			plugin.Result("{{c}}", "200", "text/plain", fnFuture, "future\n"),
			plugin.Result("{{c}}", "200", "application/x-nope", "0", "nope\n"),
			"FUNCTION_RESULT_BEGIN {{c}} 200 \"\" 0\nempty format\nFUNCTION_RESULT_END\n",
			"FUNCTION_RESULT_BEGIN {{c}} 0 text/plain 0\nzero\nFUNCTION_RESULT_END\n",
			"FUNCTION_RESULT_BEGIN {{c}} 200 text/html\n<p>no expiry</p>\nFUNCTION_RESULT_END\n",
			plugin.Result("{{c}}", "404", "application/json", fnFuture, "{\"status\":404,\"error_message\":\"plugin says no\"}\n"),
		}
		var s []plugin.Step
		for _, a := range answers {
			s = append(s, expect("c"), emit(a))
		}
		add(fnStreamCase{name: "codes",
			play: func(t *testing.T, x *fnStreamSide) []string {
				var out []string
				for i := range answers {
					out = append(out, x.do(t, fmt.Sprint("answer ", i), fnHTTPGet(call(fmt.Sprintf("function=difftest-open%%20e%d", i)), tx(21+i))))
				}
				return out
			},
			want: []string{
				fnQ("Cache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: Date+0\r\nContent-Length: 12\r\nX-Transaction-ID: " + tx(21)),
				fnQ("Cache-Control: public\r\nExpires: " + fnFutureHTTP + "\r\nContent-Length: 8\r\nX-Transaction-ID: " + tx(22)),
				fnQ("Content-Type: text/plain; charset=utf-8\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: Date+0\r\nContent-Length: 14\r\nX-Transaction-ID: " + tx(24)),
				fnQ("HTTP/1.1 591 Server Error\r\n"),
				fnQ("HTTP/1.1 404 Not Found\r\n"),
				"got a FUNCTION_RESULT_BEGIN without providing the required data (key = '" + tx(24) +
					"', status = '200', format = '', expires = '0').",
			},
		}, s...)
	}

	// the parent authorizes: its 412 and 403 never go down; the admin's and the member's sources go down whole; a
	// restricted method of the child (a `__` name, its bearer_get_token built-in) is refused on this API
	add(fnStreamCase{name: "access",
		play: func(t *testing.T, x *fnStreamSide) []string {
			admin := "Authorization: Bearer " + fnAdminToken.token
			member := "Authorization: Bearer " + fnMemberToken.token
			return []string{
				x.do(t, "anonymous fn", fnHTTPGet(call("function=difftest-fn"), tx(31))),
				x.do(t, "member fn", fnHTTPGet(call("function=difftest-fn"), tx(32), member)),
				x.do(t, "admin fn", fnHTTPGet(call("function=difftest-fn%20x"), tx(33), admin)),
				x.do(t, "anonymous hidden", fnHTTPGet(call("function=__difftest-hidden"), tx(34))),
				x.do(t, "admin hidden", fnHTTPGet(call("function=__difftest-hidden"), tx(35), admin)),
				x.do(t, "member open", fnHTTPGet(call("function=difftest-open%20m"), tx(36), member)),
				x.do(t, "anonymous token", fnHTTPGet(call("function=bearer_get_token"), tx(37))),
				x.do(t, "admin token", fnHTTPGet(call("function=bearer_get_token"), tx(38), admin)),
			}
		},
		want: []string{
			fnHTTPError(412, "You need to be authenticated via Netdata Cloud Single-Sign-On (SSO) to access this feature. "+
				"Sign-in on this dashboard, or access your Netdata via https://app.netdata.cloud."),
			fnHTTPError(403, "You need to login to the Netdata Cloud space this agent is claimed to, to access this feature."),
			`FUNCTION ` + tx(33) + ` 10 "difftest-fn x" "0x7ff" "method=api-bearer,role=admin,permissions=0x7ff,user=fnhttp-admin,` +
				`account=b6b6b6b666664666866600000000acc1,ip=localhost"` + "\n",
			fnHTTPError(412, "This feature is not available via this API."),
			fnHTTPError(403, "This feature is not available via this API."),
			`FUNCTION ` + tx(36) + ` 10 "difftest-open m" "0x9" "method=api-bearer,role=member,permissions=0x9,user=fnhttp-member,` +
				`account=b6b6b6b666664666866600000000acc2,ip=localhost"` + "\n",
		},
		wantNot: []string{"FUNCTION " + tx(31), "FUNCTION " + tx(32), "FUNCTION " + tx(34), "FUNCTION " + tx(35),
			"FUNCTION " + tx(37), "FUNCTION " + tx(38)},
	}, expect("c"), ok("c"), expect("c"), ok("c"))

	// the vnode's method through the vnode's own receiver (rrdhost_nrpc_owner of the routed host); crossed calls are
	// the parent's 404s and never go down
	add(fnStreamCase{name: "vnode",
		play: func(t *testing.T, x *fnStreamSide) []string {
			return []string{
				x.do(t, "vnode", fnHTTPGet(vcall("function=difftest-vfn%20x"), tx(41))),
				x.do(t, "vnode's on the child", fnHTTPGet(call("function=difftest-vfn"), tx(42))),
				x.do(t, "child's on the vnode", fnHTTPGet(vcall("function=difftest-open"), tx(43))),
			}
		},
		want: []string{fnHTTPLine(tx(41), 10, "difftest-vfn x"), fnQ("X-Transaction-ID: " + tx(41) + "\r\n\r\nv\n\n"),
			fnHTTPError(404, "This feature is not available on this host at this time.")},
		wantNot: []string{"FUNCTION " + tx(42), "FUNCTION " + tx(43)},
	}, expect("v"), ok("v"))

	// one transaction to the child and to its vnode, the first held: the parent's call ids are process-wide
	// (nrpc-calls.c:715-731), so the second is its 400 and never goes down
	add(fnStreamCase{name: "dup-two-hosts",
		play: func(t *testing.T, x *fnStreamSide) []string {
			first := make(chan string, 1)
			go func() { first <- x.do(t, "child", fnHTTPGet(call("function=difftest-open%20d"), tx(51))) }()
			x.step(t, x.l, "the plugin did not get the first call", fnMatched(1, "d"))
			out := []string{x.do(t, "vnode, same transaction", fnHTTPGet(vcall("function=difftest-vfn%20d"), tx(51)))}
			x.release(t, x.l, "dup")
			return append(out, <-first)
		},
		want: []string{fnHTTPError(400, "Duplicate transaction."), "NRPC: duplicate call_id '" + tx(51) + "', method: 'difftest-vfn d'",
			fnHTTPLine(tx(51), 10, "difftest-open d"), fnQ("X-Transaction-ID: " + tx(51) + "\r\n\r\nd\n\n")},
		wantNot: []string{`"difftest-vfn d"`},
	}, expect("d"), plugin.Step{WaitFile: "dup"}, ok("d"))

	// a method whose plugin exited on the child stays the parent's (no FUNCTION_DEL), so the call goes down: the
	// child's 503 comes up, with the hop's newline
	add(fnStreamCase{name: "child-503",
		play: func(t *testing.T, x *fnStreamSide) []string {
			b := x.more["difftestb.plugin"]
			x.release(t, b, "gone")
			if !x.step(t, b, "difftestb did not exit and start again", startHangs(2)) {
				return nil
			}
			time.Sleep(500 * time.Millisecond)
			return []string{x.do(t, "exited", fnHTTPGet(call("function=difftest-gone"), tx(61)))}
		},
		want: []string{fnQ("Content-Length: 99\r\nX-Transaction-ID: " + tx(61) + "\r\n\r\n" +
			`{"status":503,"errorMessage":"The plugin that registered this feature, is not currently running."}` + "\n")},
	})

	topo.compare = fnStreamCatalog
	topo.sc = fnScenario(steps...)
	return topo
}

// fnStreamCatalog compares the parents' lists of the child's and its vnode's methods: `/host/<host>/api/v1/functions`
// byte for byte (the head's clock and transaction masked), the `functions` member of `/host/<host>/api/v1/info` by
// value and by place, and `/api/v2|v3/functions` through the v2 envelope (compareInfoV2), where each parent's own
// built-ins merge with the child's (since M8 commit 9); the oracle's guards.
func fnStreamCatalog(t *testing.T, p *Pair, _ [2]*fnStreamSide) {
	t.Helper()
	for _, target := range []string{fnChildHost + "/api/v1/functions", "/host/" + rvName + "/api/v1/functions"} {
		var head, body [2]string
		for i, side := range p.Each() {
			b, err := rawExchange(side.Daemon.Addr, fnHTTPGet(target, ""), fnWait)
			if err != nil {
				t.Fatalf("%s: %s: %v", side.Role, target, err)
			}
			head[i], body[i] = fnCatalogHead(b), string(httpBody(b))
		}
		if head[0] != head[1] {
			t.Errorf("%s: heads differ\noracle:    %q\ncandidate: %q", target, head[0], head[1])
		}
		if body[0] != body[1] {
			t.Errorf("%s: bodies differ\n%s", target, firstDifference([]byte("\r\n\r\n"+body[0]), []byte("\r\n\r\n"+body[1])))
		}
		// the child's re-list (stream_global_function_cb, nrpc-catalog.c:140-177) carries its built-ins and every
		// method its plugins registered, restricted ones too; the parent's list leaves the restricted out (the USER
		// filter, nrpc-catalog.c:38-48) and keeps a method whose plugin exited on the child (no FUNCTION_DEL)
		listed, absent := []string{`"difftest-vfn":{`}, []string{`"difftest-open"`}
		if target == fnChildHost+"/api/v1/functions" {
			listed = []string{`"netdata-streaming":{`, `"topology:streaming":{`, `"netdata-api-calls":{`,
				`"netdata-metrics-cardinality":{`, `"difftest-open":{`, `"difftest-fn":{`, `"difftest-gone":{`}
			absent = []string{`"__difftest-hidden"`, `"bearer_get_token"`, `"config"`, `"difftest-vfn"`}
		}
		at := -1
		for _, l := range listed {
			k := strings.Index(body[0], l)
			if k <= at {
				t.Errorf("oracle: %s lists %s at %d (after %d, in the child's order)", target, l, k, at)
			}
			at = k
		}
		for _, n := range absent {
			if strings.Contains(body[0], n) {
				t.Errorf("oracle: %s lists %s", target, n)
			}
		}
		t.Logf("oracle %s:\n%s", target, body[0])
	}
	fnCompareInfoFunctionsAt(t, p, []string{fnChildHost + "/api/v1/info", "/host/" + rvName + "/api/v1/info"})
	var requests [][2]string
	for _, target := range []string{"/api/v2/functions?options=minify", "/api/v2/functions?options=minify&scope_nodes=" +
		rchildHostname, "/api/v2/functions?options=minify&nodes=" + rvName, fnChildHost + "/api/v2/functions?options=minify",
		"/api/v3/functions?options=minify"} {
		requests = append(requests, [2]string{target, "200"})
	}
	compareInfoV2(t, p, "", requests)
	b, err := rawExchange(p.Oracle.Addr, fnHTTPGet("/api/v2/functions?options=minify", ""), fnWait)
	if err != nil {
		t.Fatalf("oracle: %v", err)
	}
	body := string(httpBody(b))
	t.Logf("oracle /api/v2/functions?options=minify: %s", body)
	// the merge (api_v2_contexts.c:688-700, keyed `<version>|<name>`): each user-visible built-in is one item with the
	// parent's localhost (0, created first) beside the child (1), in C's order before the child's plugin methods
	at := -1
	for _, n := range fnBuiltinNames {
		k := regexp.MustCompile(`\{"name":"` + regexp.QuoteMeta(n) + `","help":"[^"]*","ni":\[0,1\],`).FindStringIndex(body)
		if k == nil || k[0] <= at {
			t.Errorf("oracle: /api/v2/functions has no %q of localhost and the child after %d: %v", n, at, k)
			continue
		}
		at = k[0]
	}
	for _, w := range []string{`"nm":"parity-parent","ni":0,`, `"nm":"` + rchildHostname + `","ni":1,`, `"nm":"` + rvName + `","ni":2,`,
		`{"name":"difftest-open","help":"open fn","ni":[1],`, `{"name":"difftest-vfn","help":"vnode fn","ni":[2],`} {
		if !strings.Contains(body, w) {
			t.Errorf("oracle: /api/v2/functions has no %s", w)
		}
	}
}

// fnStreamTiming is the `timing` topology (debug level): a parent's wait timeout and GC, a client's cancel, progress.
func fnStreamTiming() fnStreamTopo {
	tx := fnStreamTx
	emit := func(s string) plugin.Step { return plugin.Step{Emit: s} }
	expect := plugin.ExpectFunction
	ok := func(name string) plugin.Step {
		return emit(plugin.Result("{{"+name+"}}", "200", "text/plain", "0", name+"\n"))
	}
	call := func(query string) string { return fnChildHost + "/api/v1/function?" + query }
	topo := fnStreamTopo{debug: true, listed: []fnListed{{fnChildHost, "difftest-open"}}}
	steps := []plugin.Step{emit(fnOpenRegister)}
	add := func(c fnStreamCase, s ...plugin.Step) {
		topo.cases = append(topo.cases, c)
		steps = append(steps, s...)
	}

	// A (1 s) unanswered: the parent's waiter 504s at its deadline + 1 s; A again at 2.5 s is the parent's duplicate;
	// B at 3 s: the parent's GC cancels A down after B's line, the child's own GC cancels A to the plugin after B's
	// line and sends A's 504 up, which the parent no longer has
	{
		a, b := tx(101), tx(102)
		add(fnStreamCase{name: "timeout",
			play: func(t *testing.T, x *fnStreamSide) []string {
				t0 := time.Now()
				out := []string{x.do(t, "A", fnHTTPGet(call("function=difftest-open%20a&timeout=1"), a))}
				out = append(out, fmt.Sprintf("cancels on stdin after A's 504: %d", strings.Count(fnStdin(x.l, 1), "FUNCTION_CANCEL")))
				fnAt(t0, 2500*time.Millisecond)
				out = append(out, x.do(t, "A again", fnHTTPGet(call("function=difftest-open%20a&timeout=1"), a)))
				fnAt(t0, 3*time.Second)
				out = append(out, x.do(t, "B", fnHTTPGet(call("function=difftest-open%20b&timeout=3"), b)))
				out = append(out, x.do(t, "A after the GC", fnHTTPGet(call("function=difftest-open%20a2&timeout=3"), a)))
				return out
			},
			want: []string{
				fnHTTPError(504, "Timeout while waiting for a response from the plugin that serves this features"),
				"cancels on stdin after A's 504: 0",
				fnHTTPError(400, "Duplicate transaction."),
				"NRPC: duplicate call_id '" + a + "', method: 'difftest-open a'",
				// P3: the child's own GC (its deadline is the line's 1 s) cancels A after B's line
				fnHTTPLine(b, 3, "difftest-open b") + "FUNCTION_CANCEL " + a + "\n" + fnHTTPLine(a, 3, "difftest-open a2"),
				"got a FUNCTION_RESULT_BEGIN for transaction '" + a + "', but the transaction is not found.",
				"NRPC: received a CANCEL request for call_id '" + a + "', but the call_id is not running.",
				fnQ("X-Transaction-ID: " + a + "\r\n\r\na2\n\n"),
			},
		}, expect("a"), expect("b"), plugin.ExpectCancel("x"), ok("b"), expect("a2"), ok("a2"))
	}

	// the client gone while the child's plugin works: the parent's 499, its FUNCTION_CANCEL down, the child's to the
	// plugin; the plugin's late answer comes up to a call nobody waits for
	for _, half := range []bool{false, true} {
		name, n, e := "cancel", 111, "k"
		if half {
			name, n, e = "cancel-half", 121, "h"
		}
		add(fnStreamCase{name: name,
			play: func(t *testing.T, x *fnStreamSide) []string {
				got, err := rawHoldAndClose(x.d.Addr, fnHTTPGet(call("function=difftest-open%20"+e), tx(n)), func() {
					x.step(t, x.l, "the plugin did not get the call", fnMatched(1, e))
				}, half, fnWait)
				if err != nil {
					t.Errorf("%s: %v", x.role, err)
				}
				out := []string{"after the close: " + strconv.Quote(fnHTTPMask(got))}
				out = append(out, fmt.Sprintf("the plugin got the cancel: %t", x.step(t, x.l, "no cancel", fnMatched(1, e+"x"))))
				// the plugin's late answer goes up to the parent's cancelled record
				time.Sleep(time.Second)
				return out
			},
			want: []string{"FUNCTION_CANCEL " + tx(n) + "\n", "the plugin got the cancel: true", "code=499"},
			// the late answer finds the parent's record (kept until its result, pluginsd_functions.c:14-72)
			wantNot: []string{"got a FUNCTION_RESULT_BEGIN for transaction '" + tx(n) + "'"},
		}, expect(e), plugin.ExpectCancel(e+"x"), ok(e))
		if half {
			// the 499 head without its body (D158.1)
			topo.cases[len(topo.cases)-1].want = append(topo.cases[len(topo.cases)-1].want,
				fnQ("Content-Length: 49\r\nX-Transaction-ID: "+tx(n)+"\r\n\r\n")+`"`)
		}
	}

	// progress: the plugin's 5 of 10 comes up to the parent's table; the parent's progress request at 1.5 s extends
	// P and goes down to the plugin; the answer at 3.5 s, past P's 2 s, is a 200
	{
		p := tx(131)
		add(fnStreamCase{name: "progress",
			play: func(t *testing.T, x *fnStreamSide) []string {
				t0 := time.Now()
				answered := make(chan string, 1)
				go func() {
					answered <- x.do(t, "P answered", fnHTTPGet(call("function=difftest-open%20p&timeout=2"), p))
				}()
				x.step(t, x.l, "the plugin did not get P", fnMatched(1, "p"))
				fnAt(t0, 1500*time.Millisecond)
				out := []string{x.report(t, "P at 1.5 s", "/api/v2/progress?transaction="+p)}
				x.step(t, x.l, "the plugin did not get the progress request", fnMatched(1, "pp"))
				fnAt(t0, 3500*time.Millisecond)
				x.release(t, x.l, "answer")
				out = append(out, <-answered)
				return append(out, x.report(t, "P finished", fnChildHost+"/api/v2/progress?transaction="+p))
			},
			want: []string{
				`P at 1.5 s: "HTTP/1.1 200 OK`, fnQ(`"progress":50}`),
				"FUNCTION_PROGRESS " + p + "\n",
				// both hops extend their own deadline (nrpc-calls.c:836-860)
				`request="/api/v2/progress?transaction=` + p + `" msg="Extending function timeout due to PROGRESS update..."`,
				`request="'FUNCTION_PROGRESS' '` + p + `'" msg="Extending function timeout due to PROGRESS update..."`,
				`P answered: "HTTP/1.1 200 OK`, fnQ("X-Transaction-ID: " + p + "\r\n\r\np\n\n"),
			},
		}, expect("p"), emit(plugin.Progress("{{p}}", 5, 10)), plugin.ExpectProgress("pp"), plugin.Step{WaitFile: "answer"},
			emit(plugin.Result("{{p}}", "200", "text/plain", "0", "p\n")))
	}
	topo.sc = fnScenario(steps...)
	return topo
}

// fnStreamDisconnect is the `disconnect` topology (debug level): the child killed while its plugin holds a call,
// then started again.
func fnStreamDisconnect() fnStreamTopo {
	tx := fnStreamTx
	emit := func(s string) plugin.Step { return plugin.Step{Emit: s} }
	expect := plugin.ExpectFunction
	call := func(query string) string { return fnChildHost + "/api/v1/function?" + query }
	a, b, r, o := tx(201), tx(202), tx(203), tx(204)
	oldRegister := `FUNCTION GLOBAL "difftest-old" 10 "first run only" "top" "0x8" 100 1` + "\n"
	topo := fnStreamTopo{debug: true, starts: 2, listed: []fnListed{{fnChildHost, "difftest-old"}},
		sc: plugin.Scenario{Starts: []plugin.Start{
			{Steps: []plugin.Step{emit(fnOpenRegister + oldRegister), expect("a"), {Hang: true}}},
			{Steps: []plugin.Step{emit(fnOpenRegister), expect("r"),
				emit(plugin.Result("{{r}}", "200", "text/plain", "0", "r\n"))}},
		}}}
	// the child killed while its plugin holds A: the parent's receiver ends, A gets the teardown's 503; the methods
	// leave the child's list and a new call is the parent's 503
	topo.cases = append(topo.cases, fnStreamCase{name: "disconnect",
		play: func(t *testing.T, x *fnStreamSide) []string {
			answered := make(chan string, 1)
			go func() {
				answered <- x.do(t, "A, the child killed", fnHTTPGet(call("function=difftest-open%20a&timeout=10"), a))
			}()
			if x.step(t, x.l, "the plugin did not get A", fnMatched(1, "a")) {
				if err := x.child.Kill(); err != nil {
					t.Errorf("%s: kill the child: %v", x.role, err)
				}
			}
			out := []string{<-answered}
			return append(out, x.do(t, "the list", fnHTTPGet(fnChildHost+"/api/v1/functions", "")),
				x.do(t, "B after the kill", fnHTTPGet(call("function=difftest-open%20b"), b)))
		},
		want: []string{
			// P4: the receiver's teardown answers before the waiter's 10 s (pluginsd_functions.c:104-105)
			fnHTTPError(503, "The plugin that was servicing this request, exited before responding."),
			fnQ("\r\n\r\n{\n    \"functions\":{\n    }\n}\n"),
			fnHTTPError(503, "The plugin that registered this feature, is not currently running."),
			"NRPC: method 'difftest-open b' is not available. host '" + rchildHostname + "'",
		},
		wantNot: []string{"FUNCTION " + b},
	})
	// the child started again: the methods it registers again are the new connection's, the call answered; one it no
	// longer registers stays in the parent's registry without a FUNCTION_DEL (nrpc-registry.c:621-622), unavailable
	// (its epoch is the old connection's): not listed, a call the parent's 503 (P7)
	topo.cases = append(topo.cases, fnStreamCase{name: "reconnect",
		play: func(t *testing.T, x *fnStreamSide) []string {
			if err := x.child.Restart(); err != nil {
				t.Errorf("%s: start the child again: %v", x.role, err)
				return nil
			}
			if !fnStreamOnline(x.d.Addr, rchildGUID, 90*time.Second) || !x.listed(t, []fnListed{{fnChildHost, "difftest-open"}}, "") {
				t.Errorf("%s: the child is not back", x.role)
				return nil
			}
			return []string{x.do(t, "after the reconnect", fnHTTPGet(call("function=difftest-open%20r"), r)),
				x.do(t, "the list", fnHTTPGet(fnChildHost+"/api/v1/functions", "")),
				x.do(t, "the first run's method", fnHTTPGet(call("function=difftest-old"), o))}
		},
		want: []string{fnHTTPLine(r, 10, "difftest-open r"), fnQ("X-Transaction-ID: " + r + "\r\n\r\nr\n\n"),
			"NRPC: method 'difftest-open' of host 0xPTR re-registered with changes", `\"difftest-open\":{`,
			fnHTTPError(503, "The plugin that registered this feature, is not currently running."),
			"NRPC: method 'difftest-old' is not available. host '" + rchildHostname + "'"},
		wantNot: []string{`\"difftest-old\"`, "FUNCTION " + o},
	})
	return topo
}
