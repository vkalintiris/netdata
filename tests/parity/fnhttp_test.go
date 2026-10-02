// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
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
	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The Functions HTTP endpoints (M8 commit 6, D157; plan evidence/2026-10-02-plan-m8-commit6.md §5): `fn.http` calls the
// fake plugin's methods over `/api/v1|v3/function`, `fn.http-catalog` compares the lists and `fn.http-progress` the
// query-progress table. Each runs the fake plugin as the plugin checks do (runPluginCases): both agents at once, every
// start's stdin compared byte for byte after both stopped.

// fnHTTPTx is a transaction an fn.http case sends (X-Transaction-Id): fnTx's scheme, so fnMaskIDs keeps it.
func fnHTTPTx(n int) string { return fnTx(0x1000 + n) }

// Methods the fake plugin registers besides fnRegister's difftest-fn (signed-in, same-space, sensitive-data):
// difftest-open needs only anonymous data, so any HTTP client may call it; a `__` name is restricted
// (nrpc-registry.c:486-488), refused on this API (nrpc-calls.c:540-545).
const (
	fnOpenRegister   = `FUNCTION GLOBAL "difftest-open" 10 "open fn" "top" "0x8" 100 1` + "\n"
	fnHiddenRegister = `FUNCTION GLOBAL "__difftest-hidden" 10 "hidden" "top" "0x0" 100 1` + "\n"
	// fnFutureHTTP is fnFuture as an HTTP date: an answer keeps it (nrpc-calls.c:432-438)
	fnFutureHTTP = "Fri, 01 Jan 2100 00:00:00 GMT"
)

// fnHTTPMask masks a response's run-to-run values, keeping its bytes otherwise: Date; Expires as its distance from
// Date (`Date+0` for a no-cache answer, web_client.c:935-944) unless it is the plugin's constant (fnFutureHTTP, kept);
// an X-Transaction-ID no case sent (C's random id, web_client.c:1470-1471).
func fnHTTPMask(resp []byte) string {
	head, body, whole := bytes.Cut(resp, []byte("\r\n\r\n"))
	lines := strings.Split(string(head), "\r\n")
	var date time.Time
	for _, l := range lines {
		if v, ok := strings.CutPrefix(l, "Date: "); ok {
			date, _ = time.Parse(http.TimeFormat, v)
		}
	}
	for i, l := range lines {
		if _, ok := strings.CutPrefix(l, "Date: "); ok {
			lines[i] = "Date: D"
		} else if v, ok := strings.CutPrefix(l, "Expires: "); ok && v != fnFutureHTTP {
			if e, err := time.Parse(http.TimeFormat, v); err == nil && !date.IsZero() {
				lines[i] = fmt.Sprintf("Expires: Date%+d", int64(e.Sub(date)/time.Second))
			}
		} else if v, ok := strings.CutPrefix(l, "X-Transaction-ID: "); ok && !strings.HasPrefix(v, fnTxPrefix) {
			lines[i] = "X-Transaction-ID: RANDOM"
		}
	}
	out := strings.Join(lines, "\r\n")
	if whole {
		out += "\r\n\r\n" + string(body)
	}
	return out
}

// fnQ is s as it shows in an exchange (quoted, without the quotes): what a case's want names in a response.
func fnQ(s string) string {
	q := strconv.Quote(s)
	return q[1 : len(q)-1]
}

// fnHTTPGet is a GET of target carrying X-Transaction-Id tx (none when empty) and more header lines.
func fnHTTPGet(target, tx string, headers ...string) []byte {
	if tx != "" {
		headers = append([]string{"X-Transaction-Id: " + tx}, headers...)
	}
	return rawRequest("GET", target, headers, nil)
}

// fnHTTPSide is one agent's run of an fn.http case: its daemon and its fake plugins' layouts.
type fnHTTPSide struct {
	role Role
	d    *daemon.Daemon
	l    plugin.Layout
	more map[string]plugin.Layout
}

// do sends one request on a connection of its own and returns `<label>: <response masked, quoted>`.
func (x *fnHTTPSide) do(t *testing.T, label string, req []byte) string {
	t.Helper()
	b, err := rawExchange(x.d.Addr, req, fnWait)
	if err != nil {
		t.Errorf("%s: %s: %v", x.role, label, err)
		return label + ": " + err.Error()
	}
	return label + ": " + strconv.Quote(fnHTTPMask(b))
}

// step waits until a fake plugin's starts satisfy ok.
func (x *fnHTTPSide) step(t *testing.T, l plugin.Layout, what string, ok func([][]plugin.Record) bool) bool {
	t.Helper()
	if _, done := l.WaitFor(fnWait, ok); done {
		return true
	}
	t.Errorf("%s: %s within %v", x.role, what, fnWait)
	return false
}

// release creates a WaitFile step's file.
func (x *fnHTTPSide) release(t *testing.T, l plugin.Layout, name string) {
	t.Helper()
	if err := l.Release(name); err != nil {
		t.Errorf("%s: %v", x.role, err)
	}
}

// fnListed is a method a case waits for: listed on `<host>/api/v1/functions` (host "" is localhost's, else
// `/host/<name>`).
type fnListed struct{ host, name string }

// listed polls `<host>/api/v1/functions?harness=wait` (with the auth header line, when set) until each method is
// listed; the polls are left out of the access records (fnHTTPProbeRe).
func (x *fnHTTPSide) listed(t *testing.T, methods []fnListed, auth string) bool {
	t.Helper()
	var headers []string
	if auth != "" {
		headers = append(headers, auth)
	}
	for _, m := range methods {
		target := m.host + "/api/v1/functions?harness=wait"
		key := []byte(`"` + m.name + `":{`)
		if !pollUntil(30*time.Second, func() bool {
			b, err := rawExchange(x.d.Addr, rawRequest("GET", target, headers, nil), 5*time.Second)
			return err == nil && bytes.Contains(b, key)
		}) {
			t.Errorf("%s: %q not listed on %s within 30 s", x.role, m.name, target)
			return false
		}
	}
	return true
}

// fnHTTPProbeRe are the launcher's readiness probes (`/api/v1/info`, `/api/v3/info` under bearer protection,
// daemon.go's launch) and the cases' harness=wait polls: how many there are is each side's timing.
var fnHTTPProbeRe = regexp.MustCompile(` src_port=(\d+) .* request="?(/api/v[13]/info|[^" ]+[?&]harness=wait)"?$`)

// fnHTTPAccess are a daemon's access records but the probes' (fnHTTPProbeRe, with their connections' debug records), in
// file order, normalized; a transaction a case sent stays (`sent:<tx>`; logMasks hide the random ones).
func fnHTTPAccess(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	lines := logLines(t, d.Opts.RunDir, "access.log")
	probes := map[string]bool{}
	for _, l := range lines {
		if m := fnHTTPProbeRe.FindStringSubmatch(l); m != nil {
			probes[m[1]] = true
		}
	}
	var out []string
	for _, l := range lines {
		if m := portRe.FindStringSubmatch(l); m != nil && probes[m[1]] {
			continue
		}
		l = strings.ReplaceAll(l, " transaction="+fnTxPrefix, " transaction=sent:"+fnTxPrefix)
		out = append(out, normalizeLog(l, d.Opts.RunDir, ""))
	}
	return out
}

// fnHTTPCallRecords are a daemon's call records (fnStreamRecordRe: nRPC's, the transport's) written by threads other
// than the fake plugins' and PLUGINSD (runPluginCases compares those per class): the web workers', in file order,
// normalized.
func fnHTTPCallRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if th := threadOf(l); !strings.HasPrefix(th, "PD[") && th != "PLUGINSD" && fnStreamRecordRe.MatchString(l) {
			out = append(out, fnMaskRecord(normalizeLog(l, d.Opts.RunDir, "")))
		}
	}
	return out
}

// fnHTTPCase is one fn.http scenario.
type fnHTTPCase struct {
	// sc is difftest's scenario; more are other fake plugins, enabled by enable
	sc     plugin.Scenario
	more   []morePlugin
	enable []string
	// logs is the [logs] section (the debug level); adjust and prepare as pluginCase's (bearer protection, tokens)
	logs    string
	adjust  func(o *daemon.Options)
	prepare func(t *testing.T, runDir string)
	// listed are the methods each side must list before the case plays (difftest-open when empty), polled with the
	// header line auth when set
	listed []fnListed
	auth   string
	// play runs on each side at once (t.Errorf only) and returns its exchanges, compared between the sides
	play func(t *testing.T, x *fnHTTPSide) []string
	// want are texts the oracle's exchanges, plugin stdin, access records, call records or plugin log classes must
	// hold (each by substring); wantNot are texts none of them may hold
	want, wantNot []string
	// starts is how many starts of difftest the oracle must show (1 when 0)
	starts int
}

// runFnHTTPCases plays each case through runPluginCases: both sides list the case's methods, then play at once; their
// exchanges are compared, then (both stopped) every start's view and the plugin log classes (runPluginCases), the
// access records and the web workers' call records, and the oracle's guards.
func runFnHTTPCases(t *testing.T, cases map[string]fnHTTPCase) {
	pcs := map[string]pluginCase{}
	for name, c := range cases {
		var obs [2][]string
		var oracleStdin []string
		var oracleClasses map[string][]string
		listed := c.listed
		if len(listed) == 0 {
			listed = []fnListed{{"", "difftest-open"}}
		}
		pcs[name] = pluginCase{
			sc: c.sc, more: c.more, enable: c.enable, logs: c.logs, adjust: c.adjust, prepare: c.prepare,
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				var played [2]bool
				var wg sync.WaitGroup
				for i, side := range p.Each() {
					x := &fnHTTPSide{role: side.Role, d: side.Daemon, l: ls[i], more: map[string]plugin.Layout{}}
					for _, m := range c.more {
						x.more[m.file] = moreLayouts(p, m.dir, m.file)[i]
					}
					wg.Add(1)
					go func() {
						defer wg.Done()
						if x.listed(t, listed, c.auth) {
							obs[i], played[i] = c.play(t, x), true
						}
					}()
				}
				wg.Wait()
				if !played[0] {
					t.Fatal("oracle: the case did not play")
				}
				diffLines(t, "exchanges", obs[0], obs[1])
				t.Logf("oracle exchanges:\n%s", strings.Join(obs[0], "\n"))
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if want := max(c.starts, 1); len(starts) != want {
					t.Errorf("oracle: %d starts of %s, want %d", len(starts), plugin.Name, want)
				}
				oracleStdin = nil
				for _, s := range starts {
					oracleStdin = append(oracleStdin, plugin.ViewOf(s).Stdin)
				}
				oracleClasses = classes
			},
			after: func(t *testing.T, p *Pair) {
				var access, calls [2][]string
				for i, side := range p.Each() {
					access[i] = fnHTTPAccess(t, side.Daemon)
					calls[i] = fnHTTPCallRecords(t, side.Daemon)
				}
				diffLines(t, "access records", access[0], access[1])
				diffLines(t, "web workers' call records", calls[0], calls[1])
				hay := slices.Concat(obs[0], oracleStdin, access[0], calls[0])
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
				t.Logf("oracle stdin:\n%s\naccess records:\n%s\ncall records:\n%s", strings.Join(oracleStdin, "\n"),
					strings.Join(access[0], "\n"), strings.Join(calls[0], "\n"))
			},
		}
	}
	runPluginCases(t, pcs)
}

// fnScenario is one start of difftest playing steps, then hanging until the stop.
func fnScenario(steps ...plugin.Step) plugin.Scenario {
	return plugin.Scenario{Starts: []plugin.Start{{Steps: steps}}}
}

// fnEcho registers the methods, then answers n calls in turn, each 200 text/plain `ok` with no expiry.
func fnEcho(register string, n int) plugin.Scenario {
	steps := []plugin.Step{{Emit: register}}
	for range n {
		steps = append(steps, plugin.ExpectFunction("c"),
			plugin.Step{Emit: plugin.Result("{{c}}", "200", "text/plain", "0", "ok\n")})
	}
	return fnScenario(steps...)
}

// fnHTTPLine is the call line the plugin reads for a call from an anonymous local client (user-auth.c:23-44: C's
// client address 127.0.0.1 is named `localhost` at the accept, socket.c:667-669).
func fnHTTPLine(tx string, timeout int, cmd string) string {
	return fmt.Sprintf(`FUNCTION %s %d "%s" "0x8" "method=none,role=any,permissions=0x8,ip=localhost"`+"\n", tx, timeout, cmd)
}

// fnHTTPError is an nRPC error body (json-c-parser-inline.c:43-53) as it shows in an exchange (fnQ).
func fnHTTPError(code int, msg string) string {
	return fnQ(fmt.Sprintf(`{"status":%d,"errorMessage":"%s"}`, code, msg))
}

// The bearer tokens of the access cases (bearer_test.go's format and production signature, D96.4): an admin with
// every access, and a member signed in with anonymous data (enough for the endpoint, not for a same-space method).
var (
	fnAdminToken = bearerToken{token: "b6b6b6b6-6666-4666-8666-000000000001", account: "b6b6b6b6-6666-4666-8666-00000000acc1",
		name: "fnhttp-admin", access: accessNames, role: "admin", created: 1700000000, expires: 4102444800}
	fnMemberToken = bearerToken{token: "b6b6b6b6-6666-4666-8666-000000000002", account: "b6b6b6b6-6666-4666-8666-00000000acc2",
		name: "fnhttp-member", access: []string{"signed-in", "anonymous-data"}, role: "member", created: 1700000000,
		expires: 4102444800}
)

// fnWriteTokens writes the two tokens into a run directory's `lib/bearer_tokens/` before the daemon starts.
func fnWriteTokens(t *testing.T, runDir string) {
	dir := filepath.Join(runDir, "lib", "bearer_tokens")
	if err := os.MkdirAll(dir, 0o750); err != nil {
		t.Fatal(err)
	}
	host := parentIdentity.MachineGUID
	for _, b := range []bearerToken{fnAdminToken, fnMemberToken} {
		if err := os.WriteFile(filepath.Join(dir, b.token), b.file(host, b.signature(t, host, false)), 0o640); err != nil {
			t.Fatal(err)
		}
	}
}

// TestFnHTTP (check `fn.http`, M8 commit 6, D157): the fake plugin's methods called over `/api/v1/function` and
// `/api/v3/function` (wait mode, nrpc-calls.c:377-481). Compared per case: each response's status line, headers
// (fnHTTPMask: Expires as its distance from Date) and body; every plugin start's stdin; the access records; the web
// workers' call records; the plugin threads' records (runPluginCases). `bare-timeout` runs on the candidate alone.
func TestFnHTTP(t *testing.T) {
	runFnHTTPCases(t, fnHTTPCases())
	t.Run("bare-timeout", fnHTTPBareTimeout)
}

func fnHTTPCases() map[string]fnHTTPCase {
	tx := fnHTTPTx
	emit := func(s string) plugin.Step { return plugin.Step{Emit: s} }
	expect := plugin.ExpectFunction
	ok := func(name string) plugin.Step {
		return emit(plugin.Result("{{"+name+"}}", "200", "text/plain", "0", name+"\n"))
	}
	call := func(query string) string { return "/api/v1/function?" + query }
	cases := map[string]fnHTTPCase{}

	// a call with words after the method (the plugin gets the whole decoded command), a timeout, the request's
	// transaction as the call id (api_v1_function.c:39-40), anonymous access (0x8) and C's source string; the plugin's
	// constant expiry makes the answer cacheable (nrpc-calls.c:432-438)
	cases["ok"] = fnHTTPCase{
		sc: fnScenario(emit(fnOpenRegister), expect("a"),
			emit(plugin.Result("{{a}}", "200", "application/json", fnFuture, "{\"ok\":1}\n"))),
		play: func(t *testing.T, x *fnHTTPSide) []string {
			return []string{x.do(t, "v1", fnHTTPGet(call("function=difftest-open%20a%20b&timeout=3"), tx(1)))}
		},
		want: []string{
			fnHTTPLine(tx(1), 3, "difftest-open a b"),
			fnQ("HTTP/1.1 200 OK\r\n"),
			fnQ("Cache-Control: public\r\nExpires: " + fnFutureHTTP + "\r\nContent-Length: 9\r\nX-Transaction-ID: " + tx(1)),
		},
	}

	// the same call on v3 (web_api_v3.c:166-172); v2 has no `function` command (web_api.c:100-106)
	cases["v3-v2"] = fnHTTPCase{
		sc: fnEcho(fnOpenRegister, 1),
		play: func(t *testing.T, x *fnHTTPSide) []string {
			return []string{
				x.do(t, "v3", fnHTTPGet("/api/v3/function?function=difftest-open%20v3&timeout=3", tx(11))),
				x.do(t, "v2", fnHTTPGet("/api/v2/function?function=difftest-open", tx(12))),
			}
		},
		want: []string{fnHTTPLine(tx(11), 3, "difftest-open v3"), "Unsupported API command: function"},
	}

	// what the plugin's FUNCTION_RESULT_BEGIN words become over HTTP (pluginsd_functions.c:669-704, nrpc-calls.c
	// :427-438, web_client.c:935-944): a past expiry is no-cache (Expires = Date), the constant future one is kept
	// and public; an unknown content type is text/plain, an empty one leaves the endpoint's application/json
	// (api_v1_function.c:30), a missing expiry is no-cache; status 0 is 591; a plugin's 404 with a future expiry is
	// no-cache all the same (a non-200, web_client.c:935-936)
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
		steps := []plugin.Step{emit(fnOpenRegister)}
		for _, a := range answers {
			steps = append(steps, expect("c"), emit(a))
		}
		cases["expires"] = fnHTTPCase{
			sc: fnScenario(steps...),
			play: func(t *testing.T, x *fnHTTPSide) []string {
				var out []string
				for i := range answers {
					out = append(out, x.do(t, fmt.Sprint("answer ", i), fnHTTPGet(call(fmt.Sprintf("function=difftest-open%%20e%d", i)), tx(21+i))))
				}
				return out
			},
			want: []string{
				fnQ("Cache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: Date+0\r\nContent-Length: 11\r\nX-Transaction-ID: " + tx(21)),
				fnQ("Cache-Control: public\r\nExpires: " + fnFutureHTTP + "\r\nContent-Length: 7\r\nX-Transaction-ID: " + tx(22)),
				fnQ("HTTP/1.1 591 "),
				fnQ("HTTP/1.1 404 Not Found\r\n"),
			},
		}
	}

	// a payload (url.c:340-367, pluginsd_functions.c:24-38): POST JSON as FUNCTION_PAYLOAD with its type; PUT with a
	// parameter on its type (cut at `;`); an unknown and a missing type are text/plain; an empty body is a plain
	// FUNCTION line
	cases["payload"] = fnHTTPCase{
		sc: fnScenario(emit(fnOpenRegister),
			plugin.ExpectPayload("p1"), ok("p1"), plugin.ExpectPayload("p2"), ok("p2"),
			plugin.ExpectPayload("p3"), ok("p3"), expect("p4"), ok("p4"), plugin.ExpectPayload("p5"), ok("p5")),
		play: func(t *testing.T, x *fnHTTPSide) []string {
			body := func(method, cmd, txn string, headers []string, b string) []byte {
				return rawRequest(method, call("function=difftest-open%20"+cmd), append([]string{"X-Transaction-Id: " + txn}, headers...), []byte(b))
			}
			return []string{
				x.do(t, "post json", body("POST", "j", tx(31), []string{"Content-Type: application/json"}, "{\"a\":1}\n{\"b\":2}")),
				x.do(t, "put", body("PUT", "u", tx(32), []string{"Content-Type: text/plain; charset=utf-8"}, "x=1\n")),
				x.do(t, "post unknown", body("POST", "n", tx(33), []string{"Content-Type: application/x-nope"}, "n")),
				x.do(t, "post empty", body("POST", "e", tx(34), []string{"Content-Type: application/json"}, "")),
				x.do(t, "post no type", body("POST", "z", tx(35), nil, "z")),
			}
		},
		want: []string{
			"FUNCTION_PAYLOAD " + tx(31) + ` 10 "difftest-open j" "0x8" "method=none,role=any,permissions=0x8,ip=localhost" "application/json"` +
				"\n{\"a\":1}\n{\"b\":2}\nFUNCTION_PAYLOAD_END\n",
			"FUNCTION_PAYLOAD " + tx(32) + ` 10 "difftest-open u" "0x8" "method=none,role=any,permissions=0x8,ip=localhost" "text/plain"`,
			"FUNCTION_PAYLOAD " + tx(33) + ` 10 "difftest-open n" "0x8" "method=none,role=any,permissions=0x8,ip=localhost" "text/plain"`,
			fnHTTPLine(tx(34), 10, "difftest-open e"),
			"FUNCTION_PAYLOAD " + tx(35) + ` 10 "difftest-open z" "0x8" "method=none,role=any,permissions=0x8,ip=localhost" "text/plain"`,
		},
	}

	// the endpoint's own parameter loop (api_v1_function.c:12-26) over the decoded query (web_client.c:2109-2124):
	// `&` runs and empty names skipped, the first `=` splits, the last occurrence wins, empty values included; no or an
	// empty function is 400 (:36-37); `timeout` is strtoul(v, NULL, 0) cast to int, so hex and octal parse, a
	// negative or garbage value is the method's own (nrpc-calls.c:647-649) and 2^32+3 is 3; `+` decodes to a space;
	// `%26` decodes to an `&` that splits (the FIXME at web_client.c:2114-2117); `%01` ends the decoded query
	// (url.c:221-227)
	{
		reqs := []struct{ label, query string }{
			{"empty function", "function="},
			{"no parameters", ""},
			{"last empty wins", "function=difftest-open&function="},
			{"first = splits", "function=difftest-open%20k=v&timeout=3"},
			{"control byte cut", "function=difftest-open%20a%01b&timeout=3"},
			{"hex", "function=difftest-open%20t1&timeout=0x5"},
			{"octal", "function=difftest-open%20t2&timeout=010"},
			{"negative", "function=difftest-open%20t3&timeout=-5"},
			{"garbage", "function=difftest-open%20t4&timeout=abc"},
			{"int cast", "function=difftest-open%20t5&timeout=4294967299"},
			{"last timeout wins", "function=difftest-open%20t6&timeout=5&timeout="},
			{"empty pairs", "&&function=difftest-open%20t7&&timeout=4x&"},
			{"last function wins", "function=nope&function=difftest-open%20t8&timeout=2"},
			{"plus", "function=difftest-open+t9&timeout=2"},
			{"encoded amp", "function=difftest-open%20t10%26timeout=6"},
		}
		cases["params"] = fnHTTPCase{
			sc: fnEcho(fnOpenRegister, 12),
			play: func(t *testing.T, x *fnHTTPSide) []string {
				var out []string
				for i, r := range reqs {
					target := "/api/v1/function"
					if r.query != "" {
						target += "?" + r.query
					}
					out = append(out, x.do(t, r.label, fnHTTPGet(target, tx(41+i))))
				}
				return out
			},
			want: []string{
				fnHTTPError(400, "No function given to execute."),
				fnHTTPLine(tx(44), 3, "difftest-open k=v"),
				fnHTTPLine(tx(45), 10, "difftest-open a"),
				fnHTTPLine(tx(46), 5, "difftest-open t1"),
				fnHTTPLine(tx(47), 8, "difftest-open t2"),
				fnHTTPLine(tx(48), 10, "difftest-open t3"),
				fnHTTPLine(tx(49), 10, "difftest-open t4"),
				fnHTTPLine(tx(50), 3, "difftest-open t5"),
				fnHTTPLine(tx(51), 10, "difftest-open t6"),
				fnHTTPLine(tx(52), 4, "difftest-open t7"),
				fnHTTPLine(tx(53), 2, "difftest-open t8"),
				fnHTTPLine(tx(54), 2, "difftest-open t9"),
				fnHTTPLine(tx(55), 6, "difftest-open t10"),
			},
		}
	}

	// authorization (nrpc-calls.c:504-579) with bearer tokens: anonymous gets the SSO 412, a signed-in member lacking
	// same-space the 403, an admin runs with the bearer's source (user-auth.c:23-44); a restricted method is refused on
	// this API whatever the access (412 anonymous, 403 signed-in); the member runs the open method
	cases["access"] = fnHTTPCase{
		prepare: fnWriteTokens,
		sc:      fnScenario(emit(fnOpenRegister+fnRegister+fnHiddenRegister), expect("c"), ok("c"), expect("c"), ok("c")),
		play: func(t *testing.T, x *fnHTTPSide) []string {
			admin := "Authorization: Bearer " + fnAdminToken.token
			member := "Authorization: Bearer " + fnMemberToken.token
			return []string{
				x.do(t, "anonymous fn", fnHTTPGet(call("function=difftest-fn"), tx(71))),
				x.do(t, "member fn", fnHTTPGet(call("function=difftest-fn"), tx(72), member)),
				x.do(t, "admin fn", fnHTTPGet(call("function=difftest-fn%20x"), tx(73), admin)),
				x.do(t, "anonymous hidden", fnHTTPGet(call("function=__difftest-hidden"), tx(74))),
				x.do(t, "admin hidden", fnHTTPGet(call("function=__difftest-hidden"), tx(75), admin)),
				x.do(t, "member open", fnHTTPGet(call("function=difftest-open%20m"), tx(76), member)),
			}
		},
		want: []string{
			fnHTTPError(412, "You need to be authenticated via Netdata Cloud Single-Sign-On (SSO) to access this feature. "+
				"Sign-in on this dashboard, or access your Netdata via https://app.netdata.cloud."),
			fnHTTPError(403, "You need to login to the Netdata Cloud space this agent is claimed to, to access this feature."),
			`FUNCTION ` + tx(73) + ` 10 "difftest-fn x" "0x7ff" "method=api-bearer,role=admin,permissions=0x7ff,user=fnhttp-admin,` +
				`account=b6b6b6b666664666866600000000acc1,ip=localhost"`,
			fnHTTPError(412, "This feature is not available via this API."),
			fnHTTPError(403, "This feature is not available via this API."),
			`FUNCTION ` + tx(76) + ` 10 "difftest-open m" "0x9" "method=api-bearer,role=member,permissions=0x9,user=fnhttp-member,` +
				`account=b6b6b6b666664666866600000000acc2,ip=localhost"`,
		},
	}

	// `[web] bearer token protection`: an anonymous client has no access, so the dispatcher refuses the command
	// before nRPC (web_api.c:21-28, :86-89: 412 text/plain); an admin's token runs it
	cases["access-protected"] = fnHTTPCase{
		prepare: fnWriteTokens,
		adjust:  func(o *daemon.Options) { o.WebExtra = "    bearer token protection = yes\n" },
		auth:    "Authorization: Bearer " + fnAdminToken.token,
		sc:      fnEcho(fnOpenRegister, 1),
		play: func(t *testing.T, x *fnHTTPSide) []string {
			return []string{
				x.do(t, "anonymous", fnHTTPGet(call("function=difftest-open"), tx(81))),
				x.do(t, "admin", fnHTTPGet(call("function=difftest-open%20p"), tx(82), "Authorization: Bearer "+fnAdminToken.token)),
			}
		},
		want: []string{
			fnQ("HTTP/1.1 412 Precondition Failed\r\n"),
			fnQ("Content-Type: text/plain; charset=utf-8\r\n"),
			fnQ("\r\n\r\nYou need to be authorized to access this resource"),
			`FUNCTION ` + tx(82) + ` 10 "difftest-open p" "0x7ff" "method=api-bearer,role=admin,permissions=0x7ff,user=fnhttp-admin,`,
		},
	}

	// errors (nrpc-calls.c:504-525, pluginsd_functions.c:104-105): an unknown method is 404; a method whose plugin
	// exited is 503 "not currently running" (difftestb registers it and exits); a plugin that exits on the call
	// answers 503 "exited before responding" (difftest; its next start hangs)
	{
		goneRegister := `FUNCTION GLOBAL "difftest-gone" 10 "gone" "top" "0x8" 100 1` + "\n"
		exitRegister := `FUNCTION GLOBAL "difftest-exit" 10 "exits" "top" "0x8" 100 1` + "\n"
		cases["errors"] = fnHTTPCase{
			starts: 2,
			enable: []string{"difftestb"},
			listed: []fnListed{{"", "difftest-exit"}},
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{emit(fnOpenRegister + exitRegister), expect("x"), {Exit: plugin.ExitCode(0)}}},
				{Steps: []plugin.Step{{Hang: true}}},
			}},
			more: []morePlugin{{dir: "plugins.d", file: "difftestb.plugin", sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{emit(goneRegister), {SleepMs: 300}, {Exit: plugin.ExitCode(0)}}},
				{Steps: []plugin.Step{{Hang: true}}},
			}}}},
			play: func(t *testing.T, x *fnHTTPSide) []string {
				out := []string{x.do(t, "unknown", fnHTTPGet(call("function=difftest-nothing"), tx(91)))}
				if x.step(t, x.more["difftestb.plugin"], "difftestb did not exit and start again", startHangs(2)) {
					out = append(out, x.do(t, "exited", fnHTTPGet(call("function=difftest-gone"), tx(92))))
				}
				out = append(out, x.do(t, "exits on the call", fnHTTPGet(call("function=difftest-exit"), tx(93))))
				x.step(t, x.l, "difftest did not start again", startHangs(2))
				return out
			},
			want: []string{
				fnHTTPError(404, "This feature is not available on this host at this time."),
				fnHTTPError(503, "The plugin that registered this feature, is not currently running."),
				fnHTTPError(503, "The plugin that was servicing this request, exited before responding."),
			},
		}
	}

	// the wait's timeout (nrpc-calls.c:398-405, :440-451): A (1 s) unanswered gets the waiter's 504 at its deadline
	// plus C's 1 s grace, with no CANCEL; A again at 2.5 s is a duplicate (its record waits for the plugin's or the
	// GC's answer, nrpc-calls.c:690-708); B at 3 s: its line, then the GC's FUNCTION_CANCEL A
	// (pluginsd_functions.c:494-495); A is free again and runs
	{
		a, b := tx(101), tx(102)
		cases["timeout"] = fnHTTPCase{
			logs: "    level = debug\n",
			sc: fnScenario(emit(fnOpenRegister), expect("a"), expect("b"), plugin.ExpectCancel("x"), ok("b"),
				expect("a2"), ok("a2")),
			play: func(t *testing.T, x *fnHTTPSide) []string {
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
				fnHTTPLine(b, 3, "difftest-open b") + "FUNCTION_CANCEL " + a + "\n" + fnHTTPLine(a, 3, "difftest-open a2"),
			},
		}
	}

	// the client gone while the plugin works (web_api.c:144-154, socket.c:86-108): closed whole after the plugin got
	// the call, the next 10 ms poll cancels it (nrpc-calls.c:411-419): FUNCTION_CANCEL to the plugin, 499 logged
	// WARNING; closed only for writing, the client reads C's answer: the 499 head without its body
	for _, half := range []bool{false, true} {
		name, n := "cancel", 111
		if half {
			name, n = "cancel-half", 121
		}
		cases[name] = fnHTTPCase{
			logs: "    level = debug\n",
			sc:   fnScenario(emit(fnOpenRegister), expect("a"), plugin.ExpectCancel("x"), ok("a")),
			play: func(t *testing.T, x *fnHTTPSide) []string {
				got, err := rawHoldAndClose(x.d.Addr, fnHTTPGet(call("function=difftest-open%20c"), tx(n)), func() {
					x.step(t, x.l, "the plugin did not get the call", fnMatched(1, "a"))
				}, half, fnWait)
				if err != nil {
					t.Errorf("%s: %v", x.role, err)
				}
				out := []string{"after the close: " + strconv.Quote(fnHTTPMask(got))}
				out = append(out, fmt.Sprintf("the plugin got the cancel: %t", x.step(t, x.l, "no cancel", fnMatched(1, "x"))))
				// the access record is written once the 499 went out
				time.Sleep(500 * time.Millisecond)
				return out
			},
			want: []string{"FUNCTION_CANCEL " + tx(n) + "\n", "the plugin got the cancel: true", "level=warning",
				"code=499"},
		}
	}

	// the source string (user-auth.c:23-44): a second request on a keep-alive connection has no `ip=` (the client's
	// address is cleared with the first request's state, web_client.c:220); X-Forwarded-For is cut at 45 bytes
	// (http_header.c:154-158)
	cases["source"] = fnHTTPCase{
		sc: fnEcho(fnOpenRegister, 3),
		play: func(t *testing.T, x *fnHTTPSide) []string {
			rs, err := rawExchanges(x.d.Addr, [][]byte{
				fnHTTPGet(call("function=difftest-open%20k1"), tx(131), "Connection: keep-alive"),
				fnHTTPGet(call("function=difftest-open%20k2"), tx(132), "Connection: keep-alive"),
			}, fnWait)
			if err != nil {
				t.Errorf("%s: %v", x.role, err)
			}
			var out []string
			for i, r := range rs {
				out = append(out, fmt.Sprintf("keep-alive %d: %s", i+1, strconv.Quote(fnHTTPMask(r))))
			}
			return append(out, x.do(t, "forwarded", fnHTTPGet(call("function=difftest-open%20f"), tx(133),
				"X-Forwarded-For: "+strings.Repeat("abcdef0123", 6))))
		},
		want: []string{
			fnHTTPLine(tx(131), 10, "difftest-open k1"),
			`FUNCTION ` + tx(132) + ` 10 "difftest-open k2" "0x8" "method=none,role=any,permissions=0x8"` + "\n",
			`FUNCTION ` + tx(133) + ` 10 "difftest-open f" "0x8" "method=none,role=any,permissions=0x8,ip=localhost,` +
				`forwarded_for=` + strings.Repeat("abcdef0123", 4) + "abcde\"\n",
		},
	}

	// a vnode's method (rrdhost_nrpc_owner of the routed host, api_v1_function.c:46): through `/host/<vnode>/` it runs;
	// on localhost, and localhost's method through the vnode, it is unknown
	cases["vnode"] = fnHTTPCase{
		listed: []fnListed{{"", "difftest-open"}, {"/host/" + vnodeName, "difftest-vfn"}},
		sc: fnScenario(emit(fnOpenRegister+vnodeDefine(vnodeGUID, vnodeName, vnodeLabels...)+
			`FUNCTION GLOBAL "difftest-vfn" 10 "vnode fn" "top" "0x8" 100 1`+"\n"), expect("v"), ok("v")),
		play: func(t *testing.T, x *fnHTTPSide) []string {
			return []string{
				x.do(t, "vnode", fnHTTPGet("/host/"+vnodeName+call("function=difftest-vfn%20x"), tx(141))),
				x.do(t, "vnode's on localhost", fnHTTPGet(call("function=difftest-vfn"), tx(142))),
				x.do(t, "localhost's on the vnode", fnHTTPGet("/host/"+vnodeName+call("function=difftest-open"), tx(143))),
			}
		},
		want: []string{
			fnHTTPLine(tx(141), 10, "difftest-vfn x"),
			fnHTTPError(404, "This feature is not available on this host at this time."),
		},
	}
	return cases
}

// fnStdin is what start n (from 1) of a fake plugin read so far.
func fnStdin(l plugin.Layout, n int) string {
	starts, _ := l.Starts()
	if len(starts) < n {
		return ""
	}
	return plugin.ViewOf(starts[n-1]).Stdin
}

// fnHTTPBareTimeout (fn.http's `bare-timeout`, the candidate alone): `timeout` without a value. C passes NULL to
// strtoul (api_v1_function.c:24-25) and dies (SIGSEGV in the web worker, probed 2026-10-02; D135.9, DEFECTS); the
// Rust agent runs the call with the method's own timeout. C against C skips it.
func fnHTTPBareTimeout(t *testing.T) {
	bins := binaries(t)
	if bins[0] == bins[1] {
		t.Skip("candidate only: C crashes on a bare timeout (D135.9)")
	}
	engine, err := plugin.Engine()
	if err != nil {
		t.Fatal(err)
	}
	o := pluginsOptions(1, nil, nil, "")
	o.Binary, o.RunDir, o.Identity = bins[1], runDir(t, Candidate), &parentIdentity
	l, err := plugin.Install(o.RunDir, engine, fnEcho(fnOpenRegister, 1))
	if err != nil {
		t.Fatal(err)
	}
	d, err := daemon.Start(o)
	if err != nil {
		t.Fatalf("start candidate: %v", err)
	}
	t.Cleanup(func() { _ = d.Stop() })
	x := &fnHTTPSide{role: Candidate, d: d, l: l}
	if !x.listed(t, []fnListed{{"", "difftest-open"}}, "") {
		return
	}
	tx := fnHTTPTx(151)
	got := x.do(t, "bare timeout", fnHTTPGet("/api/v1/function?function=difftest-open%20b&timeout", tx))
	if !strings.Contains(got, fnQ("HTTP/1.1 200 OK\r\n")) {
		t.Errorf("candidate: %s", got)
	}
	if !x.step(t, l, "the plugin did not get the call", fnMatched(1, "c")) {
		return
	}
	if in := fnStdin(l, 1); !strings.Contains(in, fnHTTPLine(tx, 10, "difftest-open b")) {
		t.Errorf("candidate: the plugin read %q", in)
	}
}

// ---------------------------------------------------------------------------------------------------------------
// fn.http-catalog

// fnBuiltinRe finds C's user-visible localhost built-ins in a functions list (a v1 member or a v2 item, with its
// comma): a DEVIATION mask, not a C variation. The Rust agent has no built-in methods until M8 commit 9
// (web/api/functions/functions.c), which removes this mask; C's other two, bearer_get_token and config, are never
// listed (restricted, DynCfg). Both sides go through it, so C against C passes; the oracle must list all four.
var (
	fnBuiltinNames = []string{"netdata-streaming", "topology:streaming", "netdata-api-calls", "netdata-metrics-cardinality"}
	fnBuiltinRe    = regexp.MustCompile(`\s*"(?:` + strings.Join(fnBuiltinNames, "|") + `)":\{[^{}]*\},?|` +
		`\{\s*"name":"(?:` + strings.Join(fnBuiltinNames, "|") + `)"[^{}]*\},?`)
)

// fnWithoutBuiltins is a body with fnBuiltinRe's entries removed.
func fnWithoutBuiltins(b []byte) []byte { return fnBuiltinRe.ReplaceAll(b, nil) }

// fnCatalogRegister is what difftest registers, on localhost then on the vnode: methods listed in registration order
// with their access names in table order (difftest-all has every bit); restricted ones (a `__` name, a `hidden` tag,
// nrpc-registry.c:486-488) and `config` (refused at registration, the DynCfg names) are not listed; difftest-same has
// the same name and version on both hosts (one v2 entry, `ni` of both), difftest-ver another version on the vnode
// (two entries).
var fnCatalogRegister = fnOpenRegister + fnRegister + fnHiddenRegister +
	`FUNCTION GLOBAL "difftest-tagged" 10 "tagged" "hidden" "0x8" 100 1` + "\n" +
	`FUNCTION GLOBAL "config" 10 "not mine" "top" "0x8" 100 1` + "\n" +
	`FUNCTION GLOBAL "difftest-all" 20 "every bit" "logs" "0x7ff" 5 3` + "\n" +
	`FUNCTION GLOBAL "difftest-same" 10 "same on localhost" "top" "0x8" 100 1` + "\n" +
	`FUNCTION GLOBAL "difftest-ver" 10 "version 1" "top" "0x8" 100 1` + "\n" +
	vnodeDefine(vnodeGUID, vnodeName, vnodeLabels...) +
	`FUNCTION GLOBAL "difftest-same" 10 "same on the vnode" "logs" "0x13" 7 1` + "\n" +
	`FUNCTION GLOBAL "difftest-ver" 10 "version 2" "top" "0x8" 100 2` + "\n" +
	`FUNCTION GLOBAL "difftest-vonly" 10 "vnode only" "top" "0x8" 100 1` + "\n"

// fnCatalogHead is a response's head with the clock, the random transaction and the length masked (the deviation
// mask changes the length).
func fnCatalogHead(b []byte) string {
	h, _, _ := bytes.Cut(b, []byte("\r\n\r\n"))
	return string(contentLengthRe.ReplaceAll(maskRaw(h), []byte("Content-Length: <masked>")))
}

// fnMember is the top-level member key of a JSON object, with the keys before and after it ("" at the ends).
func fnMember(v Value, key string) (Value, string, string, bool) {
	for i, m := range v.Members {
		if m.Key == key {
			var before, after string
			if i > 0 {
				before = v.Members[i-1].Key
			}
			if i+1 < len(v.Members) {
				after = v.Members[i+1].Key
			}
			return m.Value, before, after, true
		}
	}
	return Value{}, "", "", false
}

// TestFnHTTPCatalog (check `fn.http-catalog`, M8 commit 6, D157): the lists of methods. `/api/v1/functions` of
// localhost and of the vnode byte for byte (nrpc-catalog.c:182-226, the USER filter :38-48), the `functions` member of
// `/api/v1/info` by value and by place, and `/api/v2|v3/functions` (contexts v2's FUNCTIONS, NODES, AGENTS and VERSIONS,
// api_v2_contexts.c:688-806, :1481-1516) through the v2 envelope (compareInfoV2). C's built-ins go through the
// labelled deviation mask fnBuiltinRe.
func TestFnHTTPCatalog(t *testing.T) {
	runPluginCases(t, map[string]pluginCase{"lists": {
		enable: []string{"difftestb"},
		// the vnode collects five blocks of a chart, so its context's version counts in `versions` whatever the
		// `nodes=` filter (query_scope.c:64-80); all five come before the comparisons, so the plugin thread's count of
		// collections at the stop is the same on both sides
		sc: fnScenario(plugin.Step{Emit: fnCatalogRegister},
			plugin.Step{Collect: &plugin.Collect{Chart: "difftest.cv", Dims: []string{"x"}, N: 5}}),
		// a method whose plugin exited is unavailable, so not listed (nrpc-catalog.c:36-38)
		more: []morePlugin{{dir: "plugins.d", file: "difftestb.plugin", sc: plugin.Scenario{Starts: []plugin.Start{
			{Steps: []plugin.Step{{Emit: `FUNCTION GLOBAL "difftest-gone" 10 "gone" "top" "0x8" 100 1` + "\n"}, {SleepMs: 300},
				{Exit: plugin.ExitCode(0)}}},
			{Steps: []plugin.Step{{Hang: true}}},
		}}}},
		play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
			gone := moreLayouts(p, "plugins.d", "difftestb.plugin")
			for i, side := range p.Each() {
				x := &fnHTTPSide{role: side.Role, d: side.Daemon, l: ls[i]}
				if !x.listed(t, []fnListed{{"", "difftest-ver"}, {"/host/" + vnodeName, "difftest-vonly"}}, "") ||
					!x.step(t, gone[i], "difftestb did not exit and start again", startHangs(2)) ||
					!x.step(t, x.l, "the vnode's five blocks were not collected", fnCollected(5)) || !fnContextsSettled(t, x) {
					t.FailNow()
				}
			}
			fnCompareV1Lists(t, p)
			fnCompareInfoFunctions(t, p)
			var requests [][2]string
			for _, q := range []string{"", "?options=minify", "?options=mcp", "?options=debug", "?nodes=" + vnodeName,
				"?nodes=" + parentIdentity.Hostname, "?scope_nodes=" + parentIdentity.Hostname, "?nodes=nomatch",
				"?scope_nodes=" + vnodeGUID + "&options=minify"} {
				requests = append(requests, [2]string{"/api/v2/functions" + q, "200"})
			}
			requests = append(requests, [2]string{"/api/v3/functions", "200"}, [2]string{"/api/v3/functions?options=minify", "200"},
				[2]string{"/host/" + vnodeName + "/api/v2/functions", "200"})
			compareInfoV2With(t, p, "", requests, fnWithoutBuiltins)
			fnGuardV2(t, p)
		},
		guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {},
	}})
}

// fnCompareV1Lists compares `/api/v1/functions` of localhost and of the vnode: the head (clock, transaction and length
// masked) and the body without C's built-ins, byte for byte; the oracle must list its four built-ins, the methods it
// accepts and none it refuses.
func fnCompareV1Lists(t *testing.T, p *Pair) {
	t.Helper()
	for _, target := range []string{"/api/v1/functions", "/host/" + vnodeName + "/api/v1/functions"} {
		var head, body [2]string
		for i, side := range p.Each() {
			b, err := rawExchange(side.Daemon.Addr, fnHTTPGet(target, ""), fnWait)
			if err != nil {
				t.Fatalf("%s: %s: %v", side.Role, target, err)
			}
			head[i], body[i] = fnCatalogHead(b), string(fnWithoutBuiltins(httpBody(b)))
			if i == 0 {
				fnGuardList(t, target, string(httpBody(b)), target == "/api/v1/functions")
			}
		}
		if head[0] != head[1] {
			t.Errorf("%s: heads differ\noracle:    %q\ncandidate: %q", target, head[0], head[1])
		}
		if body[0] != body[1] {
			t.Errorf("%s: bodies differ\n%s", target, firstDifference([]byte("\r\n\r\n"+body[0]), []byte("\r\n\r\n"+body[1])))
		}
		t.Logf("oracle %s (built-ins removed):\n%s", target, body[0])
	}
}

// fnGuardList checks the oracle's list: localhost's holds the four built-ins (the deviation mask is not stale) and
// its accepted methods in registration order; neither holds a restricted, refused or unavailable one.
func fnGuardList(t *testing.T, target, body string, localhost bool) {
	t.Helper()
	listed := []string{`"difftest-same":{`, `"difftest-ver":{`, `"difftest-vonly":{`}
	if localhost {
		listed = []string{`"difftest-open":{`, `"difftest-fn":{`, `"difftest-all":{`, `"difftest-same":{`, `"difftest-ver":{`}
		for _, n := range fnBuiltinNames {
			if !strings.Contains(body, `"`+n+`":{`) {
				t.Errorf("oracle: %s does not list the built-in %q: the deviation mask is stale", target, n)
			}
		}
	}
	at := -1
	for _, l := range listed {
		i := strings.Index(body, l)
		if i <= at {
			t.Errorf("oracle: %s lists %s at %d (after %d, in registration order)", target, l, i, at)
		}
		at = i
	}
	for _, n := range []string{"__difftest-hidden", "difftest-tagged", `"config"`, "difftest-gone"} {
		if strings.Contains(body, n) {
			t.Errorf("oracle: %s lists %s", target, n)
		}
	}
}

// fnCollected holds once difftest's first start collected n blocks.
func fnCollected(n int) func([][]plugin.Record) bool {
	return func(s [][]plugin.Record) bool {
		return len(s) >= 1 && len(slices.DeleteFunc(slices.Clone(s[0]), func(r plugin.Record) bool { return r.Kind != "collected" })) >= n
	}
}

// fnContextsHashRe is the contexts dictionaries' summed version in a v2 answer.
var fnContextsHashRe = regexp.MustCompile(`"contexts_hard_hash":(\d+)`)

// fnContextsSettled waits until the summed contexts version is non-zero (the vnode's chart made its context) and
// the same on two reads a second apart (its first collections update it).
func fnContextsSettled(t *testing.T, x *fnHTTPSide) bool {
	t.Helper()
	last := ""
	if pollUntil(30*time.Second, func() bool {
		b, err := rawExchange(x.d.Addr, fnHTTPGet("/api/v2/functions?options=minify", ""), fnWait)
		m := fnContextsHashRe.FindSubmatch(b)
		if err != nil || m == nil || string(m[1]) == "0" {
			return false
		}
		settled := string(m[1]) == last
		last = string(m[1])
		time.Sleep(750 * time.Millisecond)
		return settled
	}) {
		return true
	}
	t.Errorf("%s: contexts_hard_hash not settled above 0 within 30 s (last %q)", x.role, last)
	return false
}

// fnGuardV2 checks the oracle's `/api/v2/functions`: one entry per name and version across the hosts, with the `ni` of
// every host registering it and the first host's attributes (api_v2_contexts.c:688-700, the dictionary's conflict
// keeps the first), and the vnode-only method on the vnode's `ni` alone.
func fnGuardV2(t *testing.T, p *Pair) {
	t.Helper()
	b, err := rawExchange(p.Oracle.Addr, fnHTTPGet("/api/v2/functions?options=minify", ""), fnWait)
	if err != nil {
		t.Fatalf("oracle: %v", err)
	}
	body := string(fnWithoutBuiltins(httpBody(b)))
	t.Logf("oracle /api/v2/functions?options=minify (built-ins removed): %s", body)
	for _, w := range []string{
		`{"name":"difftest-same","help":"same on localhost","ni":[0,1],"priority":100,"version":1,"tags":"top","access":["anonymous-data"]}`,
		`{"name":"difftest-ver","help":"version 1","ni":[0],`, `{"name":"difftest-ver","help":"version 2","ni":[1],"priority":100,"version":2,`,
		`{"name":"difftest-vonly","help":"vnode only","ni":[1],`,
	} {
		if !strings.Contains(body, w) {
			t.Errorf("oracle: /api/v2/functions has no %s", w)
		}
	}
	// the vnode's context counts with `nodes=` naming localhost alone (the walk sums before the filter)
	all := fnContextsHashRe.FindStringSubmatch(body)
	b, err = rawExchange(p.Oracle.Addr, fnHTTPGet("/api/v2/functions?options=minify&nodes="+parentIdentity.Hostname, ""), fnWait)
	if err != nil {
		t.Fatalf("oracle: %v", err)
	}
	if one := fnContextsHashRe.FindSubmatch(b); all == nil || one == nil || string(one[1]) != all[1] || all[1] == "0" {
		t.Errorf("oracle: contexts_hard_hash %q for every node, %q with nodes=%s", all, one, parentIdentity.Hostname)
	}
}

// fnCompareInfoFunctions compares the `functions` member of `/api/v1/info` (localhost's and the vnode's) by value,
// without C's built-ins, and its place among the members (api_v1_info.c:132-134: after host_labels).
func fnCompareInfoFunctions(t *testing.T, p *Pair) {
	t.Helper()
	for _, target := range []string{"/api/v1/info", "/host/" + vnodeName + "/api/v1/info"} {
		var fns [2]Value
		var place [2]string
		for i, side := range p.Each() {
			b, err := rawExchange(side.Daemon.Addr, fnHTTPGet(target, ""), fnWait)
			if err != nil {
				t.Fatalf("%s: %s: %v", side.Role, target, err)
			}
			v, err := ParseJSON(fnWithoutBuiltins(httpBody(b)))
			if err != nil {
				t.Fatalf("%s: %s: %v: %s", side.Role, target, err, truncateBytes(b))
			}
			m, before, after, ok := fnMember(v, "functions")
			if !ok {
				t.Errorf("%s: %s has no functions member", side.Role, target)
				continue
			}
			fns[i], place[i] = m, before+" < functions < "+after
		}
		if place[0] != place[1] {
			t.Errorf("%s: the functions member's place: oracle %s, candidate %s", target, place[0], place[1])
		}
		for _, d := range Compare(fns[0], fns[1]) {
			t.Errorf("%s functions: %s", target, d)
		}
		t.Logf("oracle %s: %s; functions %s", target, place[0], fns[0])
	}
}

// ---------------------------------------------------------------------------------------------------------------
// fn.http-progress

// fnProgressTimesRe are a progress report's clock values (progress.c:368-386).
var fnProgressTimesRe = regexp.MustCompile(`"(started_ut|now_ut|finished_ut|age_ut)":\d+`)

// report asks `/api/v2|v3/progress` and returns `<label>: <response masked, quoted>` with the report's clock values
// and the length masked.
func (x *fnHTTPSide) report(t *testing.T, label, target string) string {
	t.Helper()
	b, err := rawExchange(x.d.Addr, fnHTTPGet(target, ""), fnWait)
	if err != nil {
		t.Errorf("%s: %s: %v", x.role, label, err)
		return label + ": " + err.Error()
	}
	s := fnHTTPMask(b)
	s = fnProgressTimesRe.ReplaceAllString(s, `"$1":U`)
	s = contentLengthRe.ReplaceAllString(s, "Content-Length: N")
	return label + ": " + strconv.Quote(s)
}

// fnStatusLine is a response's status line.
func fnStatusLine(b []byte) string {
	l, _, _ := bytes.Cut(b, []byte("\r\n"))
	return string(l)
}

// TestFnHTTPProgress (check `fn.http-progress`, M8 commit 6, D157): the query-progress table
// (libnetdata/query_progress/progress.c:1-392) and `/api/v2|v3/progress` (api_v2_progress.c:5-27). One run, the steps
// in order on both agents at once; compared: the reports (clock values masked), the call answers, every plugin
// start's stdin and the plugin threads' records (runPluginCases).
func TestFnHTTPProgress(t *testing.T) {
	a, d, r, q := fnHTTPTx(0x201), fnHTTPTx(0x202), fnHTTPTx(0x203), fnHTTPTx(0x204)
	emit := func(s string) plugin.Step { return plugin.Step{Emit: s} }
	var obs [2][]string
	runPluginCases(t, map[string]pluginCase{"progress": {
		sc: fnScenario(emit(fnOpenRegister),
			// three blocks for the data query, all collected before it, so the plugin thread's count of collections at
			// the stop is the same on both sides
			plugin.Step{Collect: &plugin.Collect{Chart: "difftest.pr", Dims: []string{"x"}, N: 3, Background: true}},
			plugin.ExpectFunction("a"), emit(plugin.Progress("{{a}}", 5, 10)), plugin.ExpectProgress("p"),
			plugin.Step{WaitFile: "answer"}, emit(plugin.Result("{{a}}", "200", "text/plain", "0", "a\n")),
			plugin.ExpectFunction("r"), plugin.Step{WaitFile: "r"}, emit(plugin.Result("{{r}}", "200", "text/plain", "0", "r\n"))),
		play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
			var wg sync.WaitGroup
			for i, side := range p.Each() {
				x := &fnHTTPSide{role: side.Role, d: side.Daemon, l: ls[i]}
				wg.Add(1)
				go func() {
					defer wg.Done()
					if x.listed(t, []fnListed{{"", "difftest-open"}}, "") {
						obs[i] = fnProgressPlay(t, x, a, d, r, q)
					}
				}()
			}
			wg.Wait()
			diffLines(t, "exchanges", obs[0], obs[1])
			t.Logf("oracle exchanges:\n%s", strings.Join(obs[0], "\n"))
		},
		guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
			want := []string{
				// the running call's report: the plugin's 5 of 10 (progress.c:317-339, :382-385)
				`A at 1.5 s: "HTTP/1.1 200 OK`, fnQ(`{"status":200,"started_ut":U,"now_ut":U,"age_ut":U,"progress":50}`),
				// the progress request reached the plugin and extended A past its 3 s (nrpc-calls.c:836-860)
				fnQ("HTTP/1.1 200 OK\r\n"),
				fnQ(`{"status":200,"started_ut":U,"finished_ut":U,"progress":100,"age_ut":U}`),
				fnQ(`{"status":404,"message":"Transaction not found"}`),
				fnQ(`"working":0}`),
			}
			hay := slices.Concat(obs[0])
			if len(starts) == 1 {
				hay = append(hay, plugin.ViewOf(starts[0]).Stdin)
			}
			for _, w := range append(want, "FUNCTION_PROGRESS "+a+"\n", "FUNCTION_PROGRESS "+r+"\n") {
				if !slices.ContainsFunc(hay, func(l string) bool { return strings.Contains(l, w) }) {
					t.Errorf("oracle: nothing holds %q", w)
				}
			}
			for _, l := range obs[0] {
				if strings.HasPrefix(l, "A answered:") && !strings.Contains(l, fnQ("HTTP/1.1 200 OK\r\n")) {
					t.Errorf("oracle: %s", l)
				}
			}
		},
	}})
}

// fnProgressPlay plays fn.http-progress on one side and returns its exchanges.
func fnProgressPlay(t *testing.T, x *fnHTTPSide, a, d, r, q string) []string {
	var out []string
	v2 := func(tx string) string { return "/api/v2/progress?transaction=" + tx }
	// A (2 s): the plugin reports 5 of 10 at once; the report at 1.5 s pings the plugin (FUNCTION_PROGRESS) and
	// extends A to now + 10 s, so the plugin's answer at 3.5 s, past A's 3 s, is a 200 and not the waiter's 504
	t0 := time.Now()
	answered := make(chan string, 1)
	go func() {
		answered <- x.do(t, "A answered", fnHTTPGet("/api/v1/function?function=difftest-open%20a&timeout=2", a))
	}()
	x.step(t, x.l, "the plugin did not get A", fnMatched(1, "a"))
	fnAt(t0, 1500*time.Millisecond)
	out = append(out, x.report(t, "A at 1.5 s", v2(a)))
	x.step(t, x.l, "the plugin did not get the progress request", fnMatched(1, "p"))
	fnAt(t0, 3500*time.Millisecond)
	x.release(t, x.l, "answer")
	out = append(out, <-answered)
	// finished: kept in the table, any spelling of the transaction, v2 and v3; unknown, bad and missing ones are 404
	// (a bad uuid leaves the null one, api_v2_progress.c:101-102)
	dashed := strings.ToUpper(a[:8] + "-" + a[8:12] + "-" + a[12:16] + "-" + a[16:20] + "-" + a[20:])
	out = append(out,
		x.report(t, "A finished", v2(a)),
		x.report(t, "A finished, v3", "/api/v3/progress?transaction="+a),
		x.report(t, "A finished, dashed", v2(dashed)),
		x.report(t, "unknown", v2(fnHTTPTx(0x2ff))),
		x.report(t, "bad uuid", v2("xyz")),
		x.report(t, "no transaction", "/api/v2/progress"),
		x.report(t, "empty transaction", "/api/v2/progress?transaction="))
	// a data query's row (web_client.c:659-665, finished at its log, :274-278)
	x.step(t, x.l, "the three blocks were not collected", fnCollected(3))
	if b, err := rawExchange(x.d.Addr, fnHTTPGet("/api/v3/data?contexts=difftest.pr&after=-5&points=1", d), fnWait); err != nil {
		t.Errorf("%s: data: %v", x.role, err)
	} else {
		out = append(out, "data: "+fnStatusLine(b))
	}
	out = append(out, x.report(t, "data", v2(d)))
	// a reused transaction resets its finished row (progress.c:197-206): R's first request finishes, then a call with
	// R is running again
	out = append(out, x.do(t, "R first", fnHTTPGet("/api/v1/nope", r)), x.report(t, "R finished", v2(r)))
	reused := make(chan string, 1)
	go func() { reused <- x.do(t, "R reused", fnHTTPGet("/api/v1/function?function=difftest-open%20r", r)) }()
	x.step(t, x.l, "the plugin did not get R", fnMatched(1, "r"))
	out = append(out, x.report(t, "R running", v2(r)))
	x.release(t, x.l, "r")
	out = append(out, <-reused, x.report(t, "R finished again", v2(r)))
	// retention: 200 finished rows (progress.c:5, :220-227, :287-296): Q, then 198 requests; the 199th (a report) finds
	// Q, the 200th's start takes Q's row
	out = append(out, x.do(t, "Q", fnHTTPGet("/api/v1/nope", q)))
	for i := range 198 {
		if _, err := rawExchange(x.d.Addr, fnHTTPGet("/api/v1/nope", ""), fnWait); err != nil {
			t.Errorf("%s: filler %d: %v", x.role, i, err)
		}
	}
	return append(out, x.report(t, "Q after 198", v2(q)), x.report(t, "Q after 199", v2(q)))
}
