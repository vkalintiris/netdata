// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"io/fs"
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
	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// DynCfg (M8 commit 8, plan evidence/2026-10-02-plan-m8-commit8.md §5): `dyncfg.api` drives the fake plugin's
// configuration nodes through `/api/v1|v3/config` and `/api/v1|v3/function`; `dyncfg.handback` the saved files across
// plugin and agent restarts (dyncfg_handback_test.go); `dyncfg.stream` the streaming side (dyncfg_stream_test.go).
// With `[health] enabled = no` C registers no node of its own (health/health.c:160-175): every node is the fake
// plugin's, answered by its Serve step (the echoes' count and order follow saved state).

// dcTx is a transaction a DynCfg case sends (X-Transaction-Id, the call id, api_v1_config.c:38-39): fnTx's scheme, so
// the masks keep it.
func dcTx(n int) string { return fnTx(0x5000 + n) }

// The fake plugin's usual nodes, in libnetdata's quoting (functions_evloop.c:456-466): a single, a template and a stock
// job of it, all open to anonymous clients (view and edit 0x8, so nRPC's method access is 0x8, dyncfg.c:438-451).
const (
	dcS  = "difftest:s"
	dcT  = "difftest:t"
	dcJ1 = "difftest:t:j1"
	// dcJobCmds are the commands the plugin registers a job with after a user's add (scripts.d's set, plan §3.10)
	dcJobCmds = "get schema update enable disable restart test userconfig remove"
)

var (
	dcCreateS  = plugin.ConfigCreate(dcS, "accepted", "single", "/difftest", "internal", "internal", "get schema update enable disable restart", 0x8, 0x8)
	dcCreateT  = plugin.ConfigCreate(dcT, "accepted", "template", "/difftest/tmpl", "internal", "internal", "add schema enable disable restart test userconfig", 0x8, 0x8)
	dcCreateJ1 = plugin.ConfigCreate(dcJ1, "accepted", "job", "/difftest/tmpl", "stock", "stock config", "get schema update enable disable restart test userconfig", 0x8, 0x8)
	// dcCreates registers the three in one write: the template before its job (dyncfg.c:406-409)
	dcCreates = dcCreateS + dcCreateT + dcCreateJ1
)

// dcOK is the plugin's success body, naming the command, so a relayed answer shows whose it is.
const dcOK = `{"status":200,"message":"difftest {{action}} {{id}}"}` + "\n"

// dcRule is a Serve rule answering commands that match re with code and body (application/json).
func dcRule(name, re, code, body string) plugin.ServeRule {
	return plugin.ServeRule{Name: name, Re: re, Code: code, Body: body}
}

// dcServe is a Serve step: a case's own rules first, then the usual answers. A user's add is answered 200 and followed by
// the job's registration and a remove by its deletion, as scripts.d does (plan §3.10); anything else unexpected is
// answered 501 so a missing rule fails fast.
func dcServe(rules ...plugin.ServeRule) plugin.Step {
	add := dcRule("add", `^config \S+ add \S+$`, "200", dcOK)
	add.Then = "CONFIG {{id}}:{{name}} create accepted job /difftest/tmpl dyncfg '{{source}}' '" + dcJobCmds + "' 0x8 0x8\n"
	remove := dcRule("remove", `^config \S+ remove$`, "200", dcOK)
	remove.Then = "CONFIG {{id}} delete\n"
	userconfig := dcRule("userconfig", `^config \S+ userconfig \S+$`, "200", "name: {{name}}\nfrom: {{id}}\n")
	userconfig.Type = "text/plain"
	return plugin.Step{Serve: &plugin.Serve{Rules: append(rules,
		dcRule("enable", `^config \S+ enable$`, "200", dcOK),
		dcRule("disable", `^config \S+ disable$`, "200", dcOK),
		dcRule("restart", `^config \S+ restart$`, "200", dcOK),
		dcRule("update", `^config \S+ update$`, "200", dcOK),
		dcRule("get", `^config \S+ get$`, "200", `{"get":"{{id}}"}`+"\n"),
		dcRule("schema", `^config \S+ schema$`, "200", `{"type":"object","title":"plugin schema of {{id}}"}`+"\n"),
		add, remove, userconfig,
		dcRule("test", `^config \S+ test( \S+)?$`, "200", dcOK),
		dcRule("unexpected", `.`, "501", `{"status":501,"message":"difftest: no rule for {{cmd}}"}`+"\n"),
	)}}
}

// dcTimes rule: a rule answering only its first n matches (the later ones go to the next rules).
func dcTimes(r plugin.ServeRule, n int) plugin.ServeRule {
	r.Times = n
	return r
}

// dcScenario is one start of difftest: the Serve step, then each emit in its own write, then it waits for the stop.
func dcScenario(emits ...string) plugin.Scenario {
	return dcScenarioServing(dcServe(), emits...)
}

// dcScenarioServing is dcScenario with a Serve step of the case's own.
func dcScenarioServing(serve plugin.Step, emits ...string) plugin.Scenario {
	steps := []plugin.Step{serve}
	for _, e := range emits {
		steps = append(steps, plugin.Step{Emit: e})
	}
	return plugin.Scenario{Starts: []plugin.Start{{Steps: steps}}}
}

// Bearer headers of the tokens fnWriteTokens lays out (admin 0x7ff, member 0x9).
var (
	dcAdmin  = "Authorization: Bearer " + fnAdminToken.token
	dcMember = "Authorization: Bearer " + fnMemberToken.token
)

// dcSide is one agent's run of a DynCfg case.
type dcSide struct {
	fnHTTPSide
}

// get sends `/api/v1/config?<query>` with transaction dcTx(n) (none when n is 0).
func (x *dcSide) get(t *testing.T, label, query string, n int, headers ...string) string {
	t.Helper()
	return x.do(t, label, fnHTTPGet("/api/v1/config?"+query, dcTxOf(n), headers...))
}

// send sends a request with a body (POST, application/json unless a header says otherwise).
func (x *dcSide) send(t *testing.T, label, target string, n int, body string, headers ...string) string {
	t.Helper()
	h := []string{"Content-Type: application/json"}
	if n != 0 {
		h = append(h, "X-Transaction-Id: "+dcTx(n))
	}
	return x.do(t, label, rawRequest("POST", target, append(h, headers...), []byte(body)))
}

// dcTxOf is dcTx(n), or none for 0.
func dcTxOf(n int) string {
	if n == 0 {
		return ""
	}
	return dcTx(n)
}

// dcTreeRe finds a node and its status in a tree (dyncfg-tree.c:20-30: type, a job's template, then status).
func dcTreeRe(id, status string) *regexp.Regexp {
	return regexp.MustCompile(`"` + regexp.QuoteMeta(id) + `":\{"type":"[a-z]+",(?:"template":"[^"]*",)?"status":"` +
		regexp.QuoteMeta(status) + `"`)
}

// until polls `/api/v1/config?<query>&harness=wait` (left out of the access records by fnHTTPProbeRe; the polls leave
// no other record, P8) until ok holds for the response.
func (x *dcSide) until(t *testing.T, what, query string, ok func(b []byte) bool) bool {
	t.Helper()
	var last []byte
	if pollUntil(15*time.Second, func() bool {
		b, err := rawExchange(x.d.Addr, fnHTTPGet("/api/v1/config?"+query+"&harness=wait", ""), 5*time.Second)
		last = b
		return err == nil && ok(b)
	}) {
		return true
	}
	t.Errorf("%s: %s within 15 s: %q", x.role, what, last)
	return false
}

// waitTree polls the anonymous tree until it shows each node with its status.
func (x *dcSide) waitTree(t *testing.T, want map[string]string) bool {
	t.Helper()
	if len(want) == 0 {
		return true
	}
	var res []*regexp.Regexp
	for id, status := range want {
		res = append(res, dcTreeRe(id, status))
	}
	return x.until(t, fmt.Sprintf("the tree did not show %v", want), "action=tree", func(b []byte) bool {
		for _, re := range res {
			if !re.Match(b) {
				return false
			}
		}
		return true
	})
}

// served waits until start n (from 1) of difftest answered k calls with the rule name.
func (x *dcSide) served(t *testing.T, n, k int, name string) bool {
	t.Helper()
	return x.step(t, x.l, fmt.Sprintf("start %d did not serve %d %q", n, k, name), func(s [][]plugin.Record) bool {
		return len(s) >= n && dcCount(plugin.ViewOf(s[n-1]).Served, name) >= k
	})
}

// dcCount counts name in served.
func dcCount(served []string, name string) int {
	n := 0
	for _, s := range served {
		if s == name {
			n++
		}
	}
	return n
}

// dcConfigDir is a run directory's DynCfg directory (`<varlib>/config`, dyncfg.c:224-230; the launcher's lib is
// `<run>/lib`).
func dcConfigDir(runDir string) string { return filepath.Join(runDir, "lib", "config") }

// files lists the DynCfg directory: its mode, then each entry by name with its kind and mode (a symlink's target)
// and each line of a file, quoted (the payload's bytes too; dyncfg-files.c:19-68).
func (x *dcSide) files(t *testing.T, label string) []string {
	t.Helper()
	return dcFiles(t, label, dcConfigDir(x.d.Opts.RunDir))
}

// dcFiles is files over a directory.
func dcFiles(t *testing.T, label, dir string) []string {
	t.Helper()
	st, err := os.Stat(dir)
	if err != nil {
		return []string{label + ": " + strings.ReplaceAll(err.Error(), dir, "<DIR>")}
	}
	out := []string{fmt.Sprintf("%s: dir mode %04o", label, st.Mode().Perm())}
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Errorf("%s: %v", label, err)
	}
	for _, e := range entries {
		p := filepath.Join(dir, e.Name())
		info, err := os.Lstat(p)
		if err != nil {
			t.Errorf("%s: %v", label, err)
			continue
		}
		head := fmt.Sprintf("%s: %s", label, e.Name())
		switch {
		case info.Mode()&fs.ModeSymlink != 0:
			target, _ := os.Readlink(p)
			out = append(out, fmt.Sprintf("%s symlink to %s", head, filepath.Base(target)))
			continue
		case info.IsDir():
			out = append(out, head+" directory")
			continue
		}
		out = append(out, fmt.Sprintf("%s mode %04o", head, info.Mode().Perm()))
		b, err := os.ReadFile(p)
		if err != nil {
			t.Errorf("%s: %v", label, err)
			continue
		}
		for _, l := range strings.SplitAfter(string(b), "\n") {
			if l != "" {
				out = append(out, head+" "+strconv.Quote(l))
			}
		}
	}
	return out
}

var (
	// dcUsecRe and dcSecRe are wall-clock values: DynCfg's `*_ut` and the files' `created=`/`modified=` in µs, the
	// tree's `agent.now` in seconds (dyncfg-tree.c:44-58, dyncfg-files.c:47-48, api_v2_contexts_agents.c:11-21)
	dcUsecRe = regexp.MustCompile(`\b\d{16}\b`)
	dcSecRe  = regexp.MustCompile(`\b\d{10}\b`)
)

// dcClock renders a side's wall-clock values: each µs value within the run (around the play) as `T<k>`, k its rank
// among the side's distinct run values in all its observations (equalities and order stay compared), each second within
// it as `NOW`; values outside the run (a seeded file's constants, far in the past) stay as they are.
func dcClock(obs []string, lo, hi time.Time) []string {
	return dcClockAs("T", obs, lo, hi)
}

// dcClockAs is dcClock with the ranks named `<prefix><k>`: a phase rendered on its own (dyncfg.handback's swap,
// where one side reads files the other wrote) keeps its ranks apart from the run's.
func dcClockAs(prefix string, obs []string, lo, hi time.Time) []string {
	inUsec := func(s string) (int64, bool) {
		v, err := strconv.ParseInt(s, 10, 64)
		return v, err == nil && v >= lo.UnixMicro() && v <= hi.UnixMicro()
	}
	var values []int64
	for _, o := range obs {
		for _, m := range dcUsecRe.FindAllString(o, -1) {
			if v, ok := inUsec(m); ok {
				values = append(values, v)
			}
		}
	}
	slices.Sort(values)
	values = slices.Compact(values)
	out := make([]string, len(obs))
	for i, o := range obs {
		o = dcUsecRe.ReplaceAllStringFunc(o, func(m string) string {
			if v, ok := inUsec(m); ok {
				k, _ := slices.BinarySearch(values, v)
				return fmt.Sprintf("%s%d", prefix, k+1)
			}
			return m
		})
		out[i] = dcSecRe.ReplaceAllStringFunc(o, func(m string) string {
			if v, err := strconv.ParseInt(m, 10, 64); err == nil && v >= lo.Unix() && v <= hi.Unix() {
				return "NOW"
			}
			return m
		})
	}
	return out
}

// dcRecordRe are DynCfg's records (every text starts `DYNCFG`, plan §3.12), from any thread.
var dcRecordRe = regexp.MustCompile(`msg="DYNCFG`)

// dcRecords are a daemon's DynCfg records in daemon.log then collector.log, each in file order, normalized (the
// transaction a case sent kept as `sent:`), with fnMaskRecord's port and thread masks but not its id mask: the ids in
// these records are the harness's (transactions, bearer accounts) or C's constant MESSAGE_ID, compared as written.
func dcRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	var out []string
	for _, name := range []string{"daemon.log", "collector.log"} {
		for _, l := range logLines(t, d.Opts.RunDir, name) {
			if dcRecordRe.MatchString(l) {
				l = strings.ReplaceAll(l, " transaction="+fnTxPrefix, " transaction=sent:"+fnTxPrefix)
				l = normalizeLog(l, d.Opts.RunDir, "")
				l = anyLocalPortRe.ReplaceAllString(l, "127.0.0.1${1}P")
				l = anyDstPortRe.ReplaceAllString(l, " dst_port=P")
				out = append(out, name+" "+fnServingTidRe.ReplaceAllString(l, "tid: N"))
			}
		}
	}
	return out
}

// dcCase is one dyncfg.api scenario.
type dcCase struct {
	// sc is difftest's scenario; more, enable, logs, adjust and prepare as fnHTTPCase's (tokens are always written)
	sc      plugin.Scenario
	more    []morePlugin
	enable  []string
	logs    string
	adjust  func(o *daemon.Options)
	prepare func(t *testing.T, runDir string)
	// ready are the nodes (id → status) the anonymous tree must show before the case plays
	ready map[string]string
	// ue is the plugin's `update every` (1 when 0): how long C waits to start it again after it exits
	ue int
	// play runs on each side at once (t.Errorf only) and returns its observations; the DynCfg directory's listing is
	// added at the end, then the clock values are rendered (dcClock) and the sides compared
	play func(t *testing.T, x *dcSide) []string
	// pair, when set, plays instead of play, on the test goroutine with both sides (agent restarts, a copy from one
	// run directory to the other); it returns each side's observations, its own listing of the files included
	pair func(t *testing.T, p *Pair, xs [2]*dcSide) [2][]string
	// views, when set, rewrites each side's plugin start views before they are compared: C's run-to-run variation only
	views func(vs []plugin.View)
	// want are texts the oracle's observations, plugin stdin, access, call or DynCfg records or plugin log classes must
	// hold (by substring); wantNot texts none of them may hold
	want, wantNot []string
	// starts is how many starts of difftest the oracle must show (1 when 0)
	starts int
}

// runDynCfgCases plays each case through runPluginCases (every start's stdin compared with the agent's own call ids
// masked): both sides wait for the case's nodes, then play at once; compared are the observations, then (both
// stopped) the plugin's starts and log classes, the access records, the web workers' call records and the DynCfg
// records, and the oracle's guards.
func runDynCfgCases(t *testing.T, cases map[string]dcCase) {
	pcs := map[string]pluginCase{}
	for name, c := range cases {
		var obs [2][]string
		var oracleStdin []string
		var oracleClasses map[string][]string
		pcs[name] = pluginCase{
			sc: c.sc, ue: c.ue, more: c.more, enable: c.enable, logs: c.logs, adjust: c.adjust,
			prepare: func(t *testing.T, runDir string) {
				fnWriteTokens(t, runDir)
				if c.prepare != nil {
					c.prepare(t, runDir)
				}
			},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				var xs [2]*dcSide
				for i, side := range p.Each() {
					xs[i] = &dcSide{fnHTTPSide{role: side.Role, d: side.Daemon, l: ls[i], more: map[string]plugin.Layout{}}}
					for _, m := range c.more {
						xs[i].more[m.file] = moreLayouts(p, m.dir, m.file)[i]
					}
				}
				var played [2]bool
				if c.pair != nil {
					if ready := dcBoth(xs, func(_ int, x *dcSide) bool { return x.waitTree(t, c.ready) }); ready[0] && ready[1] {
						obs = c.pair(t, p, xs)
						played = [2]bool{true, true}
					}
				} else {
					played = dcBoth(xs, func(i int, x *dcSide) bool {
						if !x.waitTree(t, c.ready) {
							return false
						}
						obs[i] = c.play(t, x)
						return true
					})
				}
				for i, x := range xs {
					if played[i] && c.pair == nil {
						obs[i] = append(obs[i], x.files(t, "files at the end")...)
					}
				}
				if !played[0] {
					t.Fatal("oracle: the case did not play")
				}
				now := time.Now()
				for i := range obs {
					obs[i] = dcClock(obs[i], now.Add(-10*time.Minute), now.Add(time.Minute))
				}
				diffLines(t, "observations", obs[0], obs[1])
				t.Logf("oracle observations:\n%s", strings.Join(obs[0], "\n"))
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if want := max(c.starts, 1); len(starts) != want {
					t.Errorf("oracle: %d starts of %s, want %d", len(starts), plugin.Name, want)
				}
				oracleStdin = nil
				for _, s := range starts {
					oracleStdin = append(oracleStdin, maskedViews([]plugin.View{plugin.ViewOf(s)})[0].Stdin)
				}
				oracleClasses = classes
			},
			mask:  maskStopWalk,
			views: c.views,
			after: func(t *testing.T, p *Pair) {
				var access, calls, records [2][]string
				for i, side := range p.Each() {
					access[i] = fnHTTPAccess(t, side.Daemon)
					calls[i] = fnHTTPCallRecords(t, side.Daemon)
					records[i] = dcRecords(t, side.Daemon)
				}
				diffLines(t, "access records", access[0], access[1])
				diffLines(t, "web workers' call records", calls[0], calls[1])
				diffLines(t, "DynCfg records", records[0], records[1])
				hay := slices.Concat(obs[0], oracleStdin, access[0], calls[0], records[0])
				for _, lines := range oracleClasses {
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
				t.Logf("oracle stdin:\n%s\nDynCfg records:\n%s\naccess records:\n%s\ncall records:\n%s",
					strings.Join(oracleStdin, "\n"), strings.Join(records[0], "\n"), strings.Join(access[0], "\n"),
					strings.Join(calls[0], "\n"))
			},
		}
	}
	runPluginCases(t, pcs)
}

// dcBoth runs f on both sides at once (t.Errorf only) and returns what each gave.
func dcBoth[T any](xs [2]*dcSide, f func(i int, x *dcSide) T) [2]T {
	var out [2]T
	var wg sync.WaitGroup
	for i, x := range xs {
		wg.Add(1)
		go func() {
			defer wg.Done()
			out[i] = f(i, x)
		}()
	}
	wg.Wait()
	return out
}

// dcWriteFile writes a file under a run directory before its daemon starts (seeded DynCfg files, schemas).
func dcWriteFile(t *testing.T, path string, b []byte) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, b, 0o644); err != nil {
		t.Fatal(err)
	}
}

// dcHost is the agents' machine GUID as a DynCfg file names it (`host=`, compact lowercase, dyncfg-files.c:37-39):
// both sides share it (parentIdentity), so a file holds on either.
func dcHost() string { return strings.ReplaceAll(parentIdentity.MachineGUID, "-", "") }

// dcFile is a saved file's text in C's layout (dyncfg-files.c:33-65); payload empty for none.
func dcFile(id, template, path, typ, sourceType, source string, created, modified int64, userDisabled bool, saves int,
	cmds, payload string) []byte {
	var b bytes.Buffer
	fmt.Fprintf(&b, "version=1\nid=%s\n", id)
	if template != "" {
		fmt.Fprintf(&b, "template=%s\n", template)
	}
	fmt.Fprintf(&b, "host=%s\npath=%s\ntype=%s\nsource_type=%s\nsource=%s\ncreated=%d\nmodified=%d\nsync=false\n"+
		"user_disabled=%t\nsaves=%d\ncmds=%s\n", dcHost(), path, typ, sourceType, source, created, modified, userDisabled,
		saves, cmds)
	if payload != "" {
		fmt.Fprintf(&b, "content_type=application/json\ncontent_length=%d\n---\n%s", len(payload), payload)
	}
	return b.Bytes()
}
