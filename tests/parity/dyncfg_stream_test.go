// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"regexp"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/plugin"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// `dyncfg.stream` (M8 commit 8, D169; plan §5.4): DynCfg across a streaming hop. A child stands for all its DynCfg
// nodes with one synthetic `config` method in its re-lists (dyncfg.c:516-529); a parent registers it on the child's
// host and sends `/host/<child>/api/v1|v3/config` down as FUNCTION or FUNCTION_PAYLOAD (api_v1_config.c:83-101,
// nrpc-registry.c:827-892); the child runs it against its own DynCfg (its tree, its intercept, its plugin). The re-list
// line itself is compared by `stream.sender-capture` (its `fn-*` and `dc-*` variants) and `fn.stream-proxy`.

// dcStreamTx is a transaction a dyncfg.stream case sends.
func dcStreamTx(n int) string { return fnTx(0x6000 + n) }

// dcConfigLine is the line a C sender appends to a re-list when the host has an available DynCfg method and DYNCFG
// is common (dyncfg.c:516-529; command-function.c:38-40 at connect, command-begin-set-end-init.c:65-66 at the next
// collection after a change): localhost always has `config` (dyncfg-tree.c:304-323, database/rrd.c:185), so a C
// child sends it whatever its plugins register.
const dcConfigLine = `FUNCTION GLOBAL config 120 "Dynamic configuration" "config" 0x8 1000`

// dcStreamClock is dcClock over one side's observations of a case, with the window around now.
func dcStreamClock(obs []string) []string {
	now := time.Now()
	return dcClock(obs, now.Add(-10*time.Minute), now.Add(time.Minute))
}

// TestDynCfgStream (check `dyncfg.stream`, M8 commit 8, D169; plan §5.4): `parent` (C and candidate parents over
// identical C children whose fake plugin has DynCfg nodes: `/host/<child>/api/v1|v3/config` and the function route,
// runFnStream's comparisons), `wire` (a scripted child under each parent: the exact lines a parent sends down for a
// `config` call, with and without DYNCFG, and a stale child), `child` (a scripted parent above a C child and a
// candidate child: the child's answers to `config` calls, its plugin's stdin).
func TestDynCfgStream(t *testing.T) {
	t.Run("parent", func(t *testing.T) { runFnStream(t, dcStreamParent()) })
	t.Run("wire", func(t *testing.T) {
		runFnWireCases(t, dcWireCases(), [2]Role{"dcw-p-oracle", "dcw-p-candidate"}, func(name string, n int) stream.HostInfo {
			return stream.HostInfo{Hostname: "dcw-" + name, MachineGUID: fmt.Sprintf("5a1e0000-0000-4000-8000-0000000008%02x", n)}
		})
	})
	runFnCases(t, map[string]fnCase{"child": dcChildCase()})
}

// dcStreamParent is the `parent` topology: the C child's plugin registers the usual nodes (dcCreates) and answers
// the echoes and calls (dcServe); each parent waits until, through it, the child's tree shows them settled.
func dcStreamParent() fnStreamTopo {
	tx := dcStreamTx
	cfg := func(q string) string { return fnChildHost + "/api/v1/config?" + q }
	fn := func(q string) string { return fnChildHost + "/api/v1/function?" + q }
	return fnStreamTopo{
		sc: dcScenario(dcCreates),
		// a poll goes down to the child once the parent has the child's `config` (a 404 at the parent before)
		ready: func(t *testing.T, x *fnStreamSide) bool {
			res := []*regexp.Regexp{dcTreeRe(dcS, "running"), dcTreeRe(dcT, "accepted"), dcTreeRe(dcJ1, "running")}
			var last []byte
			if pollUntil(60*time.Second, func() bool {
				b, err := rawExchange(x.d.Addr, fnHTTPGet(cfg("action=tree&harness=wait"), ""), 5*time.Second)
				last = b
				if err != nil {
					return false
				}
				for _, re := range res {
					if !re.Match(b) {
						return false
					}
				}
				return true
			}) {
				return true
			}
			t.Errorf("%s: the child's tree through the parent did not settle within 60 s: %q", x.role, last)
			return false
		},
		cases: []fnStreamCase{
			// the child's tree through the parent (the child's `agent` trailer, dyncfg-tree.c:162): v1, v3, a path, an id,
			// an admin (the bearer's source and access go down whole: the dyncfg source shown)
			{name: "tree", play: func(t *testing.T, x *fnStreamSide) []string {
				return dcStreamClock([]string{
					x.do(t, "v1", fnHTTPGet(cfg("action=tree&path=/difftest"), tx(1))),
					x.do(t, "v3", fnHTTPGet(fnChildHost+"/api/v3/config?action=tree", tx(2))),
					x.do(t, "id", fnHTTPGet(cfg("action=tree&id=difftest:t"), tx(3))),
				})
			},
				want: []string{fnQ(`"agent":{"mg":"` + rchildGUID + `"`), fnQ(`"nm":"` + rchildHostname + `"`)},
			},
			// commands down to the child's intercept and its plugin: get, a POST update (the child saves it; its user-action
			// record parses the parent's source), a schema the child has no file for (forwarded to its plugin), an unknown
			// id (the child's catch-all 404), a multi-word action (K1: `config difftest:s (null)`, the child's 400)
			{name: "commands", play: func(t *testing.T, x *fnStreamSide) []string {
				return dcStreamClock([]string{
					x.do(t, "get", fnHTTPGet(cfg("action=get&id=difftest:s"), tx(11))),
					x.do(t, "update", rawRequest("POST", cfg("action=update&id=difftest:s"), []string{"X-Transaction-Id: " + tx(12),
						"Content-Type: application/json"}, []byte(`{"p":1}`))),
					x.do(t, "admin update", rawRequest("POST", cfg("action=update&id=difftest:s"), []string{"X-Transaction-Id: " + tx(13),
						"Content-Type: application/json", "Authorization: Bearer " + fnAdminToken.token}, []byte(`{"p":2}`))),
					x.do(t, "schema", fnHTTPGet(cfg("action=schema&id=difftest:t"), tx(14))),
					x.do(t, "tree after the updates", fnHTTPGet(cfg("action=tree&id=difftest:s"), tx(15))),
					x.do(t, "tree after the updates, admin", fnHTTPGet(cfg("action=tree&id=difftest:s"), tx(16),
						"Authorization: Bearer "+fnAdminToken.token)),
					x.do(t, "unknown id", fnHTTPGet(cfg("action=get&id=nope"), tx(17))),
					x.do(t, "multi-word action", fnHTTPGet(cfg("action=get%20update&id=difftest:s"), tx(18))),
				})
			},
				want: []string{
					`FUNCTION ` + tx(11) + ` `, `"config difftest:s get" "0x8" "method=none,role=any,permissions=0x8,ip=localhost`,
					`FUNCTION_PAYLOAD ` + tx(12) + ` `, `"config difftest:s update" "0x7ff" "method=api-bearer,role=admin,`,
					`"config difftest:t schema"`,
					fnHTTPError(404, "Unknown config id given."),
					fnQ(`{"status":400,"message":"dyncfg functions intercept: invalid command received"}`),
					"DYNCFG USER ACTION 'update' on 'difftest:s' by user 'fnhttp-admin'",
				},
			},
			// refused at the parent, nothing down (api_v1_config.c:46-75): an invalid id, an unknown action, an add without
			// a name
			{name: "refused", play: func(t *testing.T, x *fnStreamSide) []string {
				return []string{
					x.do(t, "invalid id", fnHTTPGet(cfg("action=get&id=a%20b"), tx(21))),
					x.do(t, "unknown action", fnHTTPGet(cfg("action=bogus&id=difftest:s"), tx(22))),
					x.do(t, "add without a name", rawRequest("POST", cfg("action=add&id=difftest:t"), []string{"X-Transaction-Id: " + tx(23),
						"Content-Type: application/json"}, []byte(`{"j":1}`))),
				}
			},
				want:    []string{fnHTTPError(400, "Invalid id"), fnHTTPError(400, "Invalid action"), fnHTTPError(400, "Invalid name")},
				wantNot: []string{"FUNCTION " + tx(21), "FUNCTION " + tx(22), "FUNCTION_PAYLOAD " + tx(23)},
			},
			// the function route to the child's `config` (ACL FUNCTIONS; nrpc strips the words to the child's method)
			{name: "function", play: func(t *testing.T, x *fnStreamSide) []string {
				return dcStreamClock([]string{
					x.do(t, "tree", fnHTTPGet(fn("function=config%20tree%20%27/difftest/tmpl%27%20%27%27"), tx(31))),
					x.do(t, "get", fnHTTPGet(fn("function=config%20difftest:s%20get"), tx(32))),
					x.do(t, "config alone", fnHTTPGet(fn("function=config"), tx(33))),
				})
			},
				want: []string{`FUNCTION ` + tx(32) + ` `, fnQ(`{"status":400,"message":"invalid function call, expected: config tree"}`)},
			},
		},
	}
}

// dcWireAnswer is a C child's answer to a `config` call: a 200 JSON span (pluginsd_function_result_begin_to_buffer's
// quoting) with one body line.
func dcWireAnswer(body string) []string {
	return []string{`FUNCTION_RESULT_BEGIN "{{tx}}" 200 "application/json" 0`, body, "FUNCTION_RESULT_END"}
}

// dcWireCases are the scripted-child cases: the child registers `config` (as a C child re-lists it) and `wire-fn`
// (whose listing tells the line arrived: the parent reads them in order; `config` is never listed, nrpc-catalog.c
// :44-48).
func dcWireCases() []fnWireCase {
	tx := func(n int) string { return dcStreamTx(0x100 + n) }
	cfg := func(h, q string) string { return "/host/" + h + "/api/v1/config?" + q }
	var cases []fnWireCase
	// the lines down for each `/config` command and the function route (pluginsd_functions.c:25-49): the timeout as
	// the parent rounds it (120, `timeout=3` raised to 10, api_v1_config.c:29-33), the user's access and source, a
	// payload block; a refused request sends nothing. `nodyncfg`: the same from a child without DYNCFG (the parent
	// registers a streamed `config` whatever the capability, nrpc-registry.c:565-591)
	for _, v := range []struct {
		name string
		caps uint32
		n    int
	}{{"dyncfg", fnWireCaps | stream.CapDynCfg, 0}, {"nodyncfg", fnWireCaps, 20}} {
		cases = append(cases, fnWireCase{name: v.name,
			play: func(t *testing.T, x *fnWireSide) []string {
				if !x.connect(t, v.caps, dcConfigLine+"\n"+fnWireRegister, false) {
					return nil
				}
				h := x.host.Hostname
				admin := "Authorization: Bearer " + fnAdminToken.token
				out := []string{
					x.call(t, "tree", fnHTTPGet(cfg(h, "action=tree"), tx(v.n+1)), dcWireAnswer(`{"version":1,"tree":{}}`)...),
					x.call(t, "get", fnHTTPGet(cfg(h, "action=get&id=wire:s"), tx(v.n+2)), dcWireAnswer(`{"w":1}`)...),
					x.call(t, "update", rawRequest("POST", cfg(h, "action=update&id=wire:s"), []string{"X-Transaction-Id: " + tx(v.n+3),
						"Content-Type: application/json"}, []byte(`{"w":2}`)), dcWireAnswer(`{"status":200,"message":""}`)...),
					x.call(t, "add", rawRequest("POST", cfg(h, "action=add&id=wire:t&name=n1"), []string{"X-Transaction-Id: " + tx(v.n+4),
						"Content-Type: application/json", admin}, []byte(`{"w":3}`)), dcWireAnswer(`{"status":200,"message":""}`)...),
					x.call(t, "timeout 3", fnHTTPGet(cfg(h, "action=get&id=wire:s&timeout=3"), tx(v.n+5)), dcWireAnswer(`{"w":1}`)...),
					x.call(t, "test without a name", rawRequest("POST", cfg(h, "action=test&id=wire:t:j9"), []string{"X-Transaction-Id: " + tx(v.n+6),
						"Content-Type: application/json"}, []byte(`{"w":4}`)), dcWireAnswer(`{"status":200,"message":""}`)...),
					x.call(t, "function", fnHTTPGet("/host/"+h+"/api/v1/function?function=config%20tree%20%27/%27%20%27%27", tx(v.n+7)),
						dcWireAnswer(`{"version":1,"tree":{}}`)...),
					x.do(t, "invalid id", fnHTTPGet(cfg(h, "action=get&id=a%20b"), tx(v.n+8))),
				}
				return append(out, x.downLines()...)
			},
			want: []string{
				`down "FUNCTION ` + tx(v.n+1) + ` 120 \"config tree '/' ''\" \"0x8\" \"method=none,role=any,permissions=0x8,ip=localhost\""`,
				`down "FUNCTION_PAYLOAD ` + tx(v.n+3) + ` 120 \"config wire:s update\" \"0x8\"`,
				`down "FUNCTION_PAYLOAD ` + tx(v.n+4) + ` 120 \"config wire:t add n1\" \"0x7ff\"`,
				`down "FUNCTION ` + tx(v.n+5) + ` 10 \"config wire:s get\"`,
				`down "FUNCTION_PAYLOAD ` + tx(v.n+6) + ` 120 \"config wire:t test j9\"`,
				fnHTTPError(400, "Invalid id"),
			},
			wantNot: []string{`down "FUNCTION ` + tx(v.n+8), `down "FUNCTION_PAYLOAD ` + tx(v.n+8)},
		})
	}
	// a stale `config` (dc.api.child_stale): the child gone, the parent keeps its archived host and the method, now
	// unavailable: 503 "…is not currently running." (nrpc-registry.c:899-907), nothing down
	cases = append(cases, fnWireCase{name: "stale",
		play: func(t *testing.T, x *fnWireSide) []string {
			if !x.connect(t, fnWireCaps|stream.CapDynCfg, dcConfigLine+"\n"+fnWireRegister, false) {
				return nil
			}
			h := x.host.Hostname
			out := []string{x.call(t, "before", fnHTTPGet(cfg(h, "action=tree"), tx(41)), dcWireAnswer(`{"version":1,"tree":{}}`)...)}
			_ = x.c.Close()
			if !fnStreamHasChild(x.d.Addr, x.host.MachineGUID, false, 30*time.Second) {
				t.Errorf("%s: the parent still has the child 30 s after it left", x.role)
				return out
			}
			out = append(out,
				x.do(t, "tree", fnHTTPGet(cfg(h, "action=tree"), tx(42))),
				x.do(t, "get", fnHTTPGet(cfg(h, "action=get&id=wire:s"), tx(43))),
				x.do(t, "function", fnHTTPGet("/host/"+h+"/api/v1/function?function=config%20tree", tx(44))))
			return append(out, x.downLines()...)
		},
		want:    []string{fnHTTPError(503, "The plugin that registered this feature, is not currently running.")},
		wantNot: []string{`down "FUNCTION ` + tx(42), `down "FUNCTION ` + tx(43), `down "FUNCTION ` + tx(44)},
	})
	return cases
}

// dcChildCase is the `child` case: a scripted parent above the child (fn.child's fnStart: the fake plugin registers
// difftest-fn, whose re-list the parent waits for, and the usual DynCfg nodes); once the child's own tree settled, the
// parent's `config` calls one after another, each after the previous answer.
func dcChildCase() fnCase {
	tx := func(n int) string { return dcStreamTx(0x200 + n) }
	src := "method=none,role=any,permissions=0x8,ip=localhost"
	return fnCase{
		clock: true,
		sc:    plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{dcServe(), {Emit: fnRegister + dcCreates}}}}},
		play: func(t *testing.T, x *fnSide) []string {
			own := &dcSide{fnHTTPSide{role: x.role, d: x.d, l: x.l}}
			if !own.waitTree(t, dcReady) {
				return nil
			}
			call := func(n int, cmd, access string) {
				x.send(t, fnCall(tx(n), 120, cmd, access, src))
				x.answered(t, tx(n), 1)
			}
			call(1, "config tree '/' ''", "0x8")
			call(2, "config tree '/difftest/tmpl' 'difftest:t'", "0x8")
			call(3, "config difftest:s get", "0x8")
			x.send(t, `FUNCTION_PAYLOAD `+tx(4)+` 120 "config difftest:s update" "0x8" "`+src+`" "application/json"`, `{"c":1}`,
				`FUNCTION_PAYLOAD_END`)
			x.answered(t, tx(4), 1)
			call(5, "config difftest:s schema", "0x8")
			call(6, "config nope get", "0x8")
			call(7, "config tree '/' 'a b'", "0x8")
			call(8, "config difftest:s get", "0x0")
			call(9, "config tree '/' 'difftest:s'", "0x7ff")
			return nil
		},
		want: []string{
			tx(1) + `: RESULT 200 "application/json"`,
			`FUNCTION ` + tx(3) + ` 120 "config difftest:s get" "0x8" "` + src + `"`,
			// the child adds a line before the end, as for any method (fn.stream's payload)
			`FUNCTION_PAYLOAD ` + tx(4) + ` 120 "config difftest:s update" "0x8" "` + src + `" "application/json"` + "\n{\"c\":1}\n\nFUNCTION_PAYLOAD_END\n",
			tx(6) + `: RESULT 404 "application/json" NOW+1 ["{\"status\":404,\"errorMessage\":\"Unknown config id given.\"}"]`,
			tx(7) + `: RESULT 400 "application/json" 0 ["{\"status\":400,\"message\":\"invalid id given\"}"]`,
			tx(8) + `: RESULT 412 "application/json" NOW+1`,
		},
	}
}
