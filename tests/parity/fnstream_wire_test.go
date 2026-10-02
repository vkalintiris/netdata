// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"os"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// `fn.stream-wire` (M8 commit 7, D164 B6; plan §5.3): a scripted child under each parent, so the lines a parent sends
// down are compared exactly and the child's answers can be what no C child sends: a missing capability, malformed or
// late answers, a span cut by a disconnect, a reconnect that has not re-listed its methods yet.

// fnWireCaps are a scripted child's capabilities: live data, FUNCTIONS, PROGRESS and FUNCTION_DEL (a C child offers
// the three, stream-capabilities.c:101-141).
const fnWireCaps = stream.CapsLive | stream.CapFunctions | stream.CapProgress | stream.CapFunctionDel

// fnWireRegister is the scripted child's method: 7 s its own timeout (a call's timeout of 0 takes it,
// nrpc-calls.c:647-649), anonymous data.
const fnWireRegister = `FUNCTION GLOBAL "wire-fn" 7 "wire fn" "top" "0x8" 100 1`

// fnWireSide is one parent's side of a wire case: the parent (fnHTTPSide's d) and the scripted child connected to it.
type fnWireSide struct {
	*fnHTTPSide
	host stream.HostInfo
	c    *stream.Conn
	// answered counts the calls (FUNCTION and FUNCTION_PAYLOAD lines) answered so far
	answered int
}

// fnWireHost is case n's scripted host.
func fnWireHost(name string, n int) stream.HostInfo {
	return stream.HostInfo{Hostname: "wire-" + name, MachineGUID: fmt.Sprintf("5a1e0000-0000-4000-8000-0000000007%02x", n)}
}

// connect connects the scripted child with caps (reading the parent's lines in the background unless quiet) and
// registers `register` (none when empty), then waits until the parent lists it.
func (x *fnWireSide) connect(t *testing.T, caps uint32, register string, quiet bool) bool {
	t.Helper()
	c, err := stream.Connect(x.d.Addr, parentIdentity.StreamKey, x.host, caps)
	if err != nil {
		t.Errorf("%s: %s: %v", x.role, x.host.Hostname, err)
		return false
	}
	t.Cleanup(func() { _ = c.Close() })
	x.c, x.answered = c, 0
	if quiet {
		if register != "" {
			c.Linef("%s", register)
		}
		if err := c.Flush(); err != nil {
			t.Errorf("%s: %v", x.role, err)
			return false
		}
	} else {
		c.Serve(nil, 0, nil)
		if register != "" {
			x.send(t, register)
		}
	}
	return register == "" || x.listed(t, []fnListed{{"/host/" + x.host.Hostname, "wire-fn"}}, "")
}

// send writes lines up as one burst.
func (x *fnWireSide) send(t *testing.T, lines ...string) {
	t.Helper()
	if err := x.c.Burst(func() {
		for _, l := range lines {
			x.c.Linef("%s", l)
		}
	}); err != nil {
		t.Errorf("%s: send: %v", x.role, err)
	}
}

// fnWireCallRe is a call's line from the parent, its transaction the submatch.
var fnWireCallRe = regexp.MustCompile(`^FUNCTION(?:_PAYLOAD)? (\S+) `)

// calls are the transactions of the calls the parent sent down so far, in order.
func (x *fnWireSide) calls() []string {
	var out []string
	for _, d := range x.c.Downstream() {
		if m := fnWireCallRe.FindStringSubmatch(d.Line); m != nil {
			out = append(out, m[1])
		}
	}
	return out
}

// next waits for the next call down and returns its transaction.
func (x *fnWireSide) next(t *testing.T) (string, bool) {
	t.Helper()
	var tx string
	if pollUntil(fnWait, func() bool {
		calls := x.calls()
		if len(calls) > x.answered {
			tx = calls[x.answered]
			return true
		}
		return false
	}) {
		x.answered++
		return tx, true
	}
	t.Errorf("%s: no call %d down within %v", x.role, x.answered+1, fnWait)
	return "", false
}

// answer waits for the next call down and answers it with lines, `{{tx}}` its transaction.
func (x *fnWireSide) answer(t *testing.T, lines ...string) string {
	t.Helper()
	tx, ok := x.next(t)
	if ok {
		var out []string
		for _, l := range lines {
			out = append(out, strings.ReplaceAll(l, "{{tx}}", tx))
		}
		x.send(t, out...)
	}
	return tx
}

// fnWireOK is a C child's answer: a 200 span with one body line (pluginsd_function_result_begin_to_buffer's quoting).
func fnWireOK(body string) []string {
	return []string{`FUNCTION_RESULT_BEGIN "{{tx}}" 200 "text/plain" 0`, body, "FUNCTION_RESULT_END"}
}

// call sends req on a connection of its own while the child answers the call it causes; returns the exchange.
func (x *fnWireSide) call(t *testing.T, label string, req []byte, answer ...string) string {
	t.Helper()
	got := make(chan string, 1)
	go func() { got <- x.do(t, label, req) }()
	x.answer(t, answer...)
	return <-got
}

// downLines are every line the parent sent the scripted child, random ids masked.
func (x *fnWireSide) downLines() []string {
	var out []string
	for _, d := range x.c.Downstream() {
		out = append(out, "down "+strconv.Quote(fnMaskIDs(d.Line)))
	}
	return out
}

// fnWireConcurrent tells a record of the `overflow` case's host.
func fnWireConcurrent(l string) bool {
	return strings.Contains(l, "/host/wire-overflow/") || strings.Contains(l, "node=wire-overflow ")
}

// fnWireCase is one scripted-child case: each side plays it (t.Errorf only) on its own host.
type fnWireCase struct {
	name string
	// long runs it only with PARITY_LONG
	long bool
	play func(t *testing.T, x *fnWireSide) []string
	// want and wantNot as fnStreamCase's, over the oracle's observations and the parent's records
	want, wantNot []string
}

// TestFnStreamWire (check `fn.stream-wire`, M8 commit 7, D164 B6): a C parent and a candidate parent, each under a
// scripted child (stream.Conn); the cases play in order on both sides at once, each on a host of its own. Compared per
// case: the HTTP exchanges and every line the parent sent the child; after all, the parents' access records and call
// records per thread (fn.stream's).
func TestFnStreamWire(t *testing.T) {
	cases := fnWireCases()
	bins := binaries(t)
	p := startPairWith(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, LogsExtra: "    level = debug\n",
		PulseOff: true}, parentIdentity, bins, [2]string{}, [2]Role{"fnw-p-oracle", "fnw-p-candidate"}, fnWriteTokens)
	long := os.Getenv("PARITY_LONG") == "1"
	var obs [2][][]string
	var wg sync.WaitGroup
	for i, side := range p.Each() {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for n, c := range cases {
				if c.long && !long {
					obs[i] = append(obs[i], nil)
					continue
				}
				x := &fnWireSide{fnHTTPSide: &fnHTTPSide{role: side.Role, d: side.Daemon}, host: fnWireHost(c.name, n)}
				obs[i] = append(obs[i], c.play(t, x))
			}
		}()
	}
	wg.Wait()
	time.Sleep(time.Second)
	played := time.Now()
	var access [2][]string
	var records [2]map[string][]string
	for i, side := range p.Each() {
		access[i] = fnStreamAccess(t, side.Daemon, played)
		records[i] = fnStreamParentRecords(t, side.Daemon, played)
	}
	hay := slices.Clone(access[0])
	for _, lines := range records[0] {
		hay = append(hay, lines...)
	}
	// `overflow`'s concurrent calls finish in any order, each answered by its waiter's 504 or by another dispatch's GC
	// first (C's race): its records are the oracle's guards only
	for i := range access {
		access[i] = slices.DeleteFunc(access[i], fnWireConcurrent)
		for class := range records[i] {
			records[i][class] = slices.DeleteFunc(records[i][class], fnWireConcurrent)
		}
	}
	for n, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if c.long && !long {
				t.Skip("PARITY_LONG unset")
			}
			if len(obs[0][n]) == 0 {
				t.Fatal("oracle: the case did not play")
			}
			diffLines(t, "exchanges and lines down", obs[0][n], obs[1][n])
			all := slices.Concat(obs[0][n], hay)
			for _, w := range c.want {
				if !slices.ContainsFunc(all, func(l string) bool { return strings.Contains(l, w) }) {
					t.Errorf("oracle: nothing holds %q", w)
				}
			}
			for _, w := range c.wantNot {
				if k := slices.IndexFunc(all, func(l string) bool { return strings.Contains(l, w) }); k >= 0 {
					t.Errorf("oracle: %q holds %q", all[k], w)
				}
			}
			t.Logf("oracle:\n%s", strings.Join(obs[0][n], "\n"))
		})
	}
	diffLines(t, "parent access records", access[0], access[1])
	for _, class := range slices.Sorted(maps.Keys(records[0])) {
		diffLines(t, "parent "+class, records[0][class], records[1][class])
	}
	for class := range records[1] {
		if _, ok := records[0][class]; !ok {
			t.Errorf("candidate only: parent %s: %q", class, records[1][class])
		}
	}
	t.Logf("oracle parent access records:\n%s", strings.Join(access[0], "\n"))
	for _, class := range slices.Sorted(maps.Keys(records[0])) {
		t.Logf("oracle parent %s:\n%s", class, strings.Join(records[0][class], "\n"))
	}
}

func fnWireCases() []fnWireCase {
	tx := func(n int) string { return fnTx(0x4000 + n) }
	fn := func(host, query string) string { return "/host/" + host + "/api/v1/function?" + query }
	var cases []fnWireCase

	// the lines down, exactly (pluginsd_functions.c:25-49): the timeout as the parent rounds it (3; 0 is the method's
	// 7), the access as `0x%x`, the source (anonymous, a bearer's, a keep-alive second request's without `ip=`, an
	// X-Forwarded-For), a `"` in the command and in the source made `'`, a payload block
	cases = append(cases, fnWireCase{name: "bytes",
		play: func(t *testing.T, x *fnWireSide) []string {
			if !x.connect(t, fnWireCaps, fnWireRegister, false) {
				return nil
			}
			h := x.host.Hostname
			admin := "Authorization: Bearer " + fnAdminToken.token
			out := []string{
				x.call(t, "timeout 3", fnHTTPGet(fn(h, "function=wire-fn%20a&timeout=3"), tx(1)), fnWireOK("a")...),
				x.call(t, "timeout 0", fnHTTPGet(fn(h, "function=wire-fn%20b&timeout=0"), tx(2)), fnWireOK("b")...),
				x.call(t, "admin", fnHTTPGet(fn(h, "function=wire-fn%20c"), tx(3), admin), fnWireOK("c")...),
				x.call(t, "quotes", fnHTTPGet(fn(h, "function=wire-fn%20d%22e"), tx(4), `X-Forwarded-For: a"b`),
					fnWireOK("d")...),
				x.call(t, "payload", rawRequest("POST", fn(h, "function=wire-fn%20p"), []string{"X-Transaction-Id: " + tx(5),
					"Content-Type: application/json"}, []byte("{\"a\":1}\n{\"b\":2}")), fnWireOK("p")...),
			}
			// a keep-alive connection's second request has no `ip=` (web_client.c:220)
			got := make(chan [][]byte, 1)
			go func() {
				rs, err := rawExchanges(x.d.Addr, [][]byte{
					fnHTTPGet(fn(h, "function=wire-fn%20k1"), tx(6), "Connection: keep-alive"),
					fnHTTPGet(fn(h, "function=wire-fn%20k2"), tx(7), "Connection: keep-alive"),
				}, fnWait)
				if err != nil {
					t.Errorf("%s: %v", x.role, err)
				}
				got <- rs
			}()
			x.answer(t, fnWireOK("k1")...)
			x.answer(t, fnWireOK("k2")...)
			for i, r := range <-got {
				out = append(out, fmt.Sprintf("keep-alive %d: %s", i+1, strconv.Quote(fnHTTPMask(r))))
			}
			return append(out, x.downLines()...)
		},
		want: []string{
			`down "FUNCTION ` + tx(1) + ` 3 \"wire-fn a\" \"0x8\" \"method=none,role=any,permissions=0x8,ip=localhost\""`,
			`down "FUNCTION ` + tx(2) + ` 7 \"wire-fn b\"`,
			`down "FUNCTION ` + tx(3) + ` 7 \"wire-fn c\" \"0x7ff\" \"method=api-bearer,role=admin,permissions=0x7ff,user=fnhttp-admin,`,
			`down "FUNCTION ` + tx(4) + ` 7 \"wire-fn d'e\"`,
			`down "FUNCTION_PAYLOAD ` + tx(5) + ` 7 \"wire-fn p\" \"0x8\" \"method=none,role=any,permissions=0x8,ip=localhost\" \"application/json\""`,
			`down "{\"a\":1}"`, `down "FUNCTION_PAYLOAD_END"`,
			`down "FUNCTION ` + tx(7) + ` 7 \"wire-fn k2\" \"0x8\" \"method=none,role=any,permissions=0x8\""`,
			fnQ("X-Transaction-ID: " + tx(1) + "\r\n\r\na\n"),
		},
	})

	// progress through the hop (pluginsd_functions.c:469-475, :715-736): the child's FUNCTION_PROGRESS reaches the
	// parent's table; a progress request extends the parent's deadline and goes down only when the child has PROGRESS
	// (`progress-off`: it has not); `progress-bare`: a FUNCTION_PROGRESS without numbers reads 0 and 0, which keep the
	// table's 5 of 10 (P8)
	for _, v := range []struct {
		name  string
		caps  uint32
		n     int
		up    string
		wants []string
	}{
		{"progress", fnWireCaps, 11, "FUNCTION_PROGRESS '{{tx}}' 5 10", nil},
		{"progress-off", fnWireCaps &^ stream.CapProgress, 21, "FUNCTION_PROGRESS '{{tx}}' 5 10", nil},
		{"progress-bare", fnWireCaps, 31, "FUNCTION_PROGRESS '{{tx}}' 5 10\nFUNCTION_PROGRESS '{{tx}}'", nil},
	} {
		p := tx(v.n)
		c := fnWireCase{name: v.name,
			play: func(t *testing.T, x *fnWireSide) []string {
				if !x.connect(t, v.caps, fnWireRegister, false) {
					return nil
				}
				t0 := time.Now()
				got := make(chan string, 1)
				go func() {
					got <- x.do(t, "P answered", fnHTTPGet(fn(x.host.Hostname, "function=wire-fn%20p&timeout=2"), p))
				}()
				x.answer(t, v.up)
				fnAt(t0, 1500*time.Millisecond)
				out := []string{x.report(t, "P at 1.5 s", "/api/v2/progress?transaction="+p)}
				if v.name == "progress-bare" {
					// a word missing is 0, which keeps the table's value (progress.c:329-333): the done alone moves
					x.send(t, "FUNCTION_PROGRESS '"+p+"' 7")
					time.Sleep(300 * time.Millisecond)
					out = append(out, x.report(t, "P after a done alone", "/api/v2/progress?transaction="+p))
				}
				fnAt(t0, 3500*time.Millisecond)
				x.send(t, strings.ReplaceAll(strings.Join(fnWireOK("p"), "\n"), "{{tx}}", p))
				out = append(out, <-got)
				return append(out, x.downLines()...)
			},
			want: []string{`P answered: "HTTP/1.1 200 OK`},
		}
		switch v.name {
		case "progress":
			c.want = append(c.want, fnQ(`"progress":50}`), `down "FUNCTION_PROGRESS `+p+`"`)
		case "progress-off":
			c.want = append(c.want, fnQ(`"progress":50}`))
			c.wantNot = []string{`down "FUNCTION_PROGRESS `}
		case "progress-bare":
			// P8: the bare report after 5 of 10 keeps 50, the done alone makes 7 of 10
			c.want = append(c.want, `P at 1.5 s: "HTTP/1.1 200 OK`, fnQ(`"progress":50}`), fnQ(`"progress":70}`))
		}
		cases = append(cases, c)
	}

	// a child without FUNCTIONS registers and is called all the same (no gate on FUNCTION, pluginsd_functions.c:502-623)
	cases = append(cases, fnWireCase{name: "nofn",
		play: func(t *testing.T, x *fnWireSide) []string {
			if !x.connect(t, fnWireCaps&^stream.CapFunctions, fnWireRegister, false) {
				return nil
			}
			out := []string{x.call(t, "call", fnHTTPGet(fn(x.host.Hostname, "function=wire-fn%20n"), tx(41)), fnWireOK("n")...)}
			return append(out, x.downLines()...)
		},
		want: []string{fnQ("X-Transaction-ID: " + tx(41) + "\r\n\r\nn\n")},
	})

	// answers a C child never sends (pluginsd_functions.c:669-713): status 0 is 591; an unknown content type
	// text/plain; a BEGIN without type and expiry logged, its span still read; a span for a transaction nobody called
	// logged and dropped; the parent's GC (A 1 s unanswered, B at 3 s: B's line, then A's CANCEL, P3) and A's late
	// answer, which the parent no longer has
	a, b := tx(55), tx(56)
	cases = append(cases, fnWireCase{name: "answers",
		play: func(t *testing.T, x *fnWireSide) []string {
			if !x.connect(t, fnWireCaps, fnWireRegister, false) {
				return nil
			}
			h := x.host.Hostname
			out := []string{
				x.call(t, "status 0", fnHTTPGet(fn(h, "function=wire-fn%20s"), tx(51)),
					`FUNCTION_RESULT_BEGIN "{{tx}}" 0 "text/plain" 0`, "zero", "FUNCTION_RESULT_END"),
				x.call(t, "unknown type", fnHTTPGet(fn(h, "function=wire-fn%20u"), tx(52)),
					`FUNCTION_RESULT_BEGIN "{{tx}}" 200 "application/x-nope" 0`, "nope", "FUNCTION_RESULT_END"),
				x.call(t, "missing words", fnHTTPGet(fn(h, "function=wire-fn%20m"), tx(53)),
					`FUNCTION_RESULT_BEGIN "{{tx}}" 200`, "missing", "FUNCTION_RESULT_END"),
			}
			x.send(t, `FUNCTION_RESULT_BEGIN "`+tx(59)+`" 200 "text/plain" 0`, "nobody's", "FUNCTION_RESULT_END")
			out = append(out, x.call(t, "after the stray span", fnHTTPGet(fn(h, "function=wire-fn%20v"), tx(54)), fnWireOK("v")...))
			t0 := time.Now()
			out = append(out, x.call(t, "A", fnHTTPGet(fn(h, "function=wire-fn%20a&timeout=1"), a)))
			fnAt(t0, 3*time.Second)
			got := make(chan string, 1)
			go func() { got <- x.do(t, "B", fnHTTPGet(fn(h, "function=wire-fn%20b&timeout=3"), b)) }()
			x.answer(t, fnWireOK("b")...)
			out = append(out, <-got)
			x.send(t, `FUNCTION_RESULT_BEGIN "`+a+`" 200 "text/plain" 0`, "late", "FUNCTION_RESULT_END")
			time.Sleep(500 * time.Millisecond)
			return append(out, x.downLines()...)
		},
		want: []string{
			fnQ("HTTP/1.1 591 Server Error\r\n"),
			fnQ("Content-Type: text/plain; charset=utf-8\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: Date+0\r\nContent-Length: 5\r\nX-Transaction-ID: " + tx(52)),
			"got a FUNCTION_RESULT_BEGIN without providing the required data (key = '" + tx(53) + "'",
			"got a FUNCTION_RESULT_BEGIN for transaction '" + tx(59) + "', but the transaction is not found.",
			fnQ("X-Transaction-ID: " + tx(54) + "\r\n\r\nv\n"),
			fnHTTPError(504, "Timeout while waiting for a response from the plugin that serves this features"),
			`down "FUNCTION ` + b + ` 3 \"wire-fn b\" \"0x8\" \"method=none,role=any,permissions=0x8,ip=localhost\""`,
			"got a FUNCTION_RESULT_BEGIN for transaction '" + a + "', but the transaction is not found.",
		},
	})
	{
		// P3's order on the wire: B's line, then the GC's FUNCTION_CANCEL A (pluginsd_functions.c:433, :494-495)
		c := &cases[len(cases)-1]
		c.want = append(c.want, `down "FUNCTION_CANCEL `+a+`"`)
	}

	// a 200 span the disconnect cuts (pluginsd_functions.c:157-168): 503 with what came of the body
	cases = append(cases, fnWireCase{name: "open-span",
		play: func(t *testing.T, x *fnWireSide) []string {
			if !x.connect(t, fnWireCaps, fnWireRegister, false) {
				return nil
			}
			got := make(chan string, 1)
			go func() { got <- x.do(t, "S", fnHTTPGet(fn(x.host.Hostname, "function=wire-fn%20s"), tx(61))) }()
			x.answer(t, `FUNCTION_RESULT_BEGIN "{{tx}}" 200 "application/json" 0`, `{"partial":`)
			time.Sleep(300 * time.Millisecond)
			down := x.downLines()
			_ = x.c.Close()
			return append([]string{<-got}, down...)
		},
		want: []string{fnQ("HTTP/1.1 503 Service Unavailable\r\n")},
	})

	// the child back on a new connection: its methods unavailable until it re-lists them (the epoch, stream-receiver.c
	// :1400; nrpc-registry.c:866-907), then called again
	cases = append(cases, fnWireCase{name: "reconnect-window",
		play: func(t *testing.T, x *fnWireSide) []string {
			if !x.connect(t, fnWireCaps, fnWireRegister, false) {
				return nil
			}
			h := x.host.Hostname
			out := []string{x.call(t, "before", fnHTTPGet(fn(h, "function=wire-fn%20a"), tx(71)), fnWireOK("a")...)}
			out = append(out, x.downLines()...)
			_ = x.c.Close()
			if !fnStreamHasChild(x.d.Addr, x.host.MachineGUID, false, 30*time.Second) {
				t.Errorf("%s: the parent still has the child 30 s after it left", x.role)
				return out
			}
			if !x.connect(t, fnWireCaps, "", false) {
				return out
			}
			out = append(out, x.do(t, "back, not re-listed", fnHTTPGet(fn(h, "function=wire-fn%20b"), tx(72))))
			x.send(t, fnWireRegister)
			if !x.listed(t, []fnListed{{"/host/" + h, "wire-fn"}}, "") {
				return out
			}
			out = append(out, x.call(t, "re-listed", fnHTTPGet(fn(h, "function=wire-fn%20c"), tx(73)), fnWireOK("c")...))
			return append(out, x.downLines()...)
		},
		want: []string{
			fnHTTPError(503, "The plugin that registered this feature, is not currently running."),
			"NRPC: method 'wire-fn b' is not available. host 'wire-reconnect-window'",
			fnQ("X-Transaction-ID: " + tx(73) + "\r\n\r\nc\n"),
		},
		wantNot: []string{"FUNCTION " + tx(72)},
	})

	// P9 and D58: a child that stops reading. A web request is at most 1 MiB (NETDATA_WEB_REQUEST_MAX_SIZE,
	// web_client.h:152), and send_to_child's buffer doubles its maximum whenever an add does not fit
	// (stream-circular-buffer.c:120-126, autoscale), so an add of under 10 MiB never fails: about 40 MiB of calls
	// queue, every caller gets the waiter's 504, the child stays connected. The guard rejects C's overflow texts.
	cases = append(cases, fnWireCase{name: "overflow", long: true,
		play: func(t *testing.T, x *fnWireSide) []string {
			if !x.connect(t, fnWireCaps, fnWireRegister, true) {
				return nil
			}
			payload := []byte(strings.Repeat("x", 1000*1000-1) + "\n")
			codes := make([]string, 40)
			var wg sync.WaitGroup
			for i := range codes {
				wg.Add(1)
				go func() {
					defer wg.Done()
					b, err := rawExchange(x.d.Addr, rawRequest("POST", fn(x.host.Hostname, "function=wire-fn%20o&timeout=1"),
						[]string{"X-Transaction-Id: " + tx(0x100+i), "Content-Type: text/plain"}, payload), fnWait)
					if err != nil {
						codes[i] = err.Error()
						return
					}
					codes[i] = fnStatusLine(b)
				}()
				if i%5 == 4 {
					wg.Wait()
				}
			}
			wg.Wait()
			slices.Sort(codes)
			out := []string{"statuses: " + strings.Join(slices.Compact(slices.Clone(codes)), ", ")}
			out = append(out, fmt.Sprintf("connected after: %t", fnStreamHasChild(x.d.Addr, x.host.MachineGUID, true, time.Second)),
				// stream-receiver.c:369-378's ERR and the failed send's (pluginsd_functions.c:455-467)
				fmt.Sprintf("an overflow logged: %t", logContains(t, x.d, "send buffer is full") ||
					logContains(t, x.d, "failed to send it to the plugin")))
			return out
		},
		want: []string{"statuses: HTTP/1.1 504 Gateway Timeout", "connected after: true", "an overflow logged: false"},
	})
	return cases
}
