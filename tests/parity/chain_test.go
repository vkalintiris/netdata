// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"maps"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The chain's identities, the same in every chain (separate processes and ports).
var (
	chainGP = daemon.Identity{Hostname: "parity-chain-gp", StreamKey: "5a1e0000-0000-4000-8000-00000000ca02",
		MachineGUID: "5a1e0000-0000-4000-8000-00000000ca01"}
	chainProxy = daemon.Identity{Hostname: "parity-chain-proxy", StreamKey: "5a1e0000-0000-4000-8000-00000000ca12",
		MachineGUID: "5a1e0000-0000-4000-8000-00000000ca11"}
	chainChild = daemon.Identity{Hostname: "parity-chain-child", StreamKey: "5a1e0000-0000-4000-8000-00000000ca22",
		MachineGUID: "5a1e0000-0000-4000-8000-00000000ca21"}
)

// chainRows is a child → proxy → grandparent chain; the form reads child-proxy-grandparent, `r` the candidate.
func chainRows(form string) []topoNode {
	impl := func(i int) int {
		if form[i] == 'r' {
			return 1
		}
		return 0
	}
	send := &daemon.StreamTo{Compression: true, Extra: "    reconnect delay = 5\n"}
	return []topoNode{
		{name: "gp", impl: impl(4), id: chainGP, stage: 0,
			opts: daemon.Options{StorageTiers: 1, StreamMemoryMode: "dbengine"}},
		{name: "p", impl: impl(2), id: chainProxy, stage: 1, to: []string{"gp"}, send: send,
			opts: daemon.Options{StorageTiers: 1}},
		{name: "c", impl: impl(0), id: chainChild, stage: 2, to: []string{"p"}, send: send,
			opts: daemon.Options{StorageTiers: 1, NoStreamKey: true}},
	}
}

// chain is one topology of the check and when its child came online, stopped and returned.
type chain struct {
	form string
	*topology
	down                      bool
	online, stopped, returned int64
}

func (c *chain) isDown() bool { return c.down }

// chainHops are the agents that hold the child: the child itself, the proxy, the grandparent.
var chainHops = []string{"c", "p", "gp"}

// The views of the chain's paths: the child and the proxy at the grandparent and at the proxy, and the child's own.
var (
	gpChildView    = [2]string{"gp", "/api/v3/stream_path?nodes=parity-chain-child&options=minify"}
	gpProxyView    = [2]string{"gp", "/api/v3/stream_path?nodes=parity-chain-proxy&options=minify"}
	proxyChildView = [2]string{"p", "/api/v3/stream_path?nodes=parity-chain-child&options=minify"}
	proxyOwnView   = [2]string{"p", "/api/v3/stream_path?nodes=parity-chain-proxy&options=minify"}
	childOwnView   = [2]string{"c", "/api/v3/stream_path?options=minify"}
	chainPathViews = [][2]string{gpChildView, gpProxyView, proxyChildView, proxyOwnView, childOwnView}
)

// chainDataCharts are the charts whose series the check compares across the hops.
var chainDataCharts = append(append([]string{}, gapCharts...), "netdata.memory")

// chainRecords are a chain's streaming records: the child's, the proxy's RECEIVER LEFT and receiver records, and
// the grandparent's receiver records, each set sorted.
func chainRecords(t *testing.T, c *chain) map[string][]string {
	t.Helper()
	return map[string][]string{
		"child":                rchildRecords(t, c.d("c")),
		"proxy left":           maskRecordSet(parentRecords(t, c.d("p"), "RECEIVER LEFT", nil)),
		"proxy receiver":       maskRecordSet(parentRecords(t, c.d("p"), "STREAM RCV", nil)),
		"grandparent receiver": maskRecordSet(parentRecords(t, c.d("gp"), "STREAM RCV", nil)),
	}
}

// startChains starts the three chains (`tweak`, when set, adjusting each one's rows), waits until each hop has its
// host online (the child starting once the proxy is online at the grandparent) and the child's own path has the
// three entries; a candidate chain that fails is marked down, the oracle's ends the test.
func startChains(t *testing.T, g *stagger, tweak func(rows []topoNode)) []*chain {
	bins := binaries(t)
	forms := []string{"c-c-c", "c-r-c", "r-c-r"}
	cs := make([]*chain, len(forms))
	errs := make([]error, len(forms))
	var wg sync.WaitGroup
	for i, form := range forms {
		// what a goroutine that ends early (a helper's Fatal) leaves: a chain that is down
		cs[i], errs[i] = &chain{form: form, down: true}, fmt.Errorf("%s did not start", form)
		wg.Add(1)
		go func() {
			defer wg.Done()
			rows := chainRows(form)
			if tweak != nil {
				tweak(rows)
			}
			tp, err := startTopology(t, g, "chain-"+form, bins, rows, func(tp *topology, stage int) error {
				if stage == 2 {
					if _, b, ok := waitHop(tp.addr("gp"), chainProxy.MachineGUID, true, 120*time.Second); !ok {
						return fmt.Errorf("%s: the grandparent never had the proxy online: %.300s", form, b)
					}
				}
				return nil
			})
			cs[i], errs[i] = &chain{form: form, topology: tp}, err
		}()
	}
	wg.Wait()
	for i, err := range errs {
		if err == nil {
			continue
		}
		if i == 0 {
			t.Fatalf("the oracle chain: %v", err)
		}
		t.Errorf("chain %s: %v", forms[i], err)
		cs[i].down = true
	}
	forEach(cs, func(c *chain) {
		launched := c.d("c").LaunchStartedAt
		for _, hop := range []struct{ at, guid string }{{"p", chainChild.MachineGUID}, {"gp", chainProxy.MachineGUID},
			{"gp", chainChild.MachineGUID}} {
			if _, b, ok := waitHop(c.addr(hop.at), hop.guid, true, 120*time.Second); !ok {
				t.Errorf("%s: %s never had %s online: %.300s", c.form, hop.at, hop.guid, b)
				c.down = true
				return
			}
			t.Logf("%s: %s has %s online %s after the child's launch", c.form, hop.at, hop.guid,
				time.Since(launched).Round(time.Second))
		}
		c.online = time.Now().Unix()
		if hops, ok := waitPathHops(c.addr("c"), "", fullPath, 30*time.Second); !ok {
			t.Errorf("%s: the child's own path has hops %v, not %v", c.form, hops, fullPath)
		}
	})
	if cs[0].down {
		t.FailNow()
	}
	return cs
}

// fullPath are the hops of the child's path through the whole chain.
var fullPath = []int{0, 1, 2}

// chainFnTx is the transaction of `stream.chain`'s call, the same in every chain (separate processes).
var chainFnTx = fnTx(0x6001)

// chainFnLeaf installs the fake plugin in each chain's child (the plugin checks' directories, scan and update every;
// its PULSE stays on): it registers difftest-open and answers one call, each start (the child's return starts it
// again).
func chainFnLeaf(rows []topoNode) {
	for i := range rows {
		if rows[i].name != "c" {
			continue
		}
		rows[i].prepare = func(o *daemon.Options) error {
			po := pluginsOptions(1, nil, nil, "")
			o.PluginsDir, o.PluginsExtra, o.ConfExtra = po.PluginsDir, po.PluginsExtra, po.ConfExtra
			engine, err := plugin.Engine()
			if err != nil {
				return err
			}
			_, err = plugin.Install(o.RunDir, engine, fnScenario(plugin.Step{Emit: fnOpenRegister},
				plugin.ExpectFunction("a"), plugin.Step{Emit: plugin.Result("{{a}}", "200", "text/plain", "0", "chain\n")}))
			return err
		}
	}
}

// chainFn is `stream.chain`'s call (M8 commit 7, D164 B6): the grandparent's `/host/<child>/api/v1/function` runs the
// child's plugin method through the proxy (each hop's receiver transport, the proxy's no-wait call). Per chain, once
// the grandparent lists the method: the exchange (fnHTTPMask) and the leaf plugin's stdin; each candidate chain's
// compared with c-c-c's, whose guards hold C's answer.
func chainFn(t *testing.T, cs []*chain) {
	exchanges := make([]string, len(cs))
	stdins := make([]string, len(cs))
	forEach(cs, func(c *chain) {
		k := slices.Index(cs, c)
		x := &fnHTTPSide{role: Role(c.form), d: c.d("gp"), l: plugin.LayoutOf(c.d("c").Opts.RunDir)}
		if !x.listed(t, []fnListed{{"/host/" + chainChild.Hostname, "difftest-open"}}, "") {
			return
		}
		exchanges[k] = x.do(t, "call", fnHTTPGet("/host/"+chainChild.Hostname+"/api/v1/function?function=difftest-open%20chain",
			chainFnTx))
		x.step(t, x.l, "the leaf's plugin did not get the call", fnMatched(1, "a"))
		stdins[k] = fnStdin(x.l, 1)
	})
	// three hops: the plugin's line, then one `\n` per hop above it (P1)
	for _, w := range []string{fnQ("HTTP/1.1 200 OK\r\n"), fnQ("X-Transaction-ID: " + chainFnTx + "\r\n\r\nchain\n\n\n")} {
		if !strings.Contains(exchanges[0], w) {
			t.Errorf("oracle: the grandparent's answer has no %q: %s", w, exchanges[0])
		}
	}
	if want := fnHTTPLine(chainFnTx, 10, "difftest-open chain"); stdins[0] != want {
		t.Errorf("oracle: the leaf's plugin read %q, want %q", stdins[0], want)
	}
	t.Logf("c-c-c: %s\nthe leaf's stdin: %q", exchanges[0], stdins[0])
	for k, c := range cs[1:] {
		if c.down {
			continue
		}
		t.Run(c.form+"/fn", func(t *testing.T) {
			if exchanges[k+1] != exchanges[0] {
				t.Errorf("the grandparent's answer:\noracle:    %s\ncandidate: %s", exchanges[0], exchanges[k+1])
			}
			if stdins[k+1] != stdins[0] {
				t.Errorf("the leaf's stdin:\noracle:    %q\ncandidate: %q", stdins[0], stdins[k+1])
			}
		})
	}
}

// TestStreamChain (check `stream.chain`, milestone 7 commit 8i, D121.2, D122.1; plan
// `evidence/2026-09-30-plan-m7-8i-3-stream-chain.md`): three child → proxy → grandparent chains run side by side,
// c-c-c (the oracle everywhere), c-r-c (a Rust proxy) and r-c-r (a Rust child and grandparent); each candidate chain
// is compared with c-c-c, and within each chain the hops' data with each other. The child starts only once the
// proxy streams to the grandparent, so the proxy's `_is_parent` there is always false (it is sent once, at the
// proxy's sender's ready). Phases: online, the child's view, steady-state views, data, a call from the grandparent to
// the child's fake plugin (chainFn, M8 commit 7), the child's leave, its return (the gap replicated through both
// hops), the records.
func TestStreamChain(t *testing.T) {
	g := &stagger{gap: 2 * time.Second}
	cs := startChains(t, g, chainFnLeaf)
	oracle := cs[0]
	full := fullPath
	for _, v := range []string{"gp", "p"} {
		if hops, ok := waitPathHops(oracle.addr(v), "&nodes=parity-chain-child", full, 10*time.Second); !ok {
			t.Fatalf("the oracle's %s path of the child has hops %v", v, hops)
		}
	}

	// P3: the steady state's views, compared with c-c-c, once the charts settle at the proxy and the grandparent
	// (a chart goes up at its next collection)
	settle := func(t *testing.T, c *chain) {
		for end := time.Now().Add(15 * time.Second); ; time.Sleep(time.Second) {
			p, gp := hopCharts(t, c.d("p"), chainChild.Hostname, false), hopCharts(t, c.d("gp"), chainChild.Hostname, false)
			if bytes.Equal(p, gp) {
				return
			}
			if time.Now().After(end) {
				t.Errorf("%s: the proxy's and the grandparent's charts of the child: %s", c.form, firstDifference(p, gp))
				return
			}
		}
	}
	var oracleCharts []byte
	var oracleInfo [2][]byte
	if !t.Run("c-c-c/steady", func(t *testing.T) {
		settle(t, oracle)
		oracleCharts = hopCharts(t, oracle.d("gp"), chainChild.Hostname, true)
		if len(oracleCharts) < 1000 {
			t.Fatalf("the grandparent's charts of the child: %.300s", oracleCharts)
		}
		for i, at := range []string{"gp", "p"} {
			b, err := streamInfoMasked(oracle.addr(at), chainChild.MachineGUID)
			if err != nil || !bytes.Contains(b, []byte(`"status":200`)) {
				t.Fatalf("%s stream_info of the child: %v %.300s", at, err, b)
			}
			oracleInfo[i] = b
		}
	}) {
		t.FailNow()
	}
	for _, c := range cs[1:] {
		if c.down {
			continue
		}
		t.Run(c.form+"/steady", func(t *testing.T) {
			topoViews(t, "steady", oracle.topology, c.topology, chainPathViews, entryTimes)
			for i, at := range []string{"gp", "p"} {
				b, err := streamInfoMasked(c.addr(at), chainChild.MachineGUID)
				if err != nil {
					t.Fatalf("%s stream_info of the child: %v", at, err)
				}
				if !bytes.Equal(oracleInfo[i], b) {
					t.Errorf("%s stream_info of the child: %s", at, firstDifference(oracleInfo[i], b))
				}
			}
			settle(t, c)
			if got := hopCharts(t, c.d("gp"), chainChild.Hostname, true); !bytes.Equal(oracleCharts, got) {
				t.Errorf("the grandparent's charts of the child: %s", firstDifference(oracleCharts, got))
			}
		})
	}

	// P4: the data within each chain, exact over the live window, tolerant from the child's first point
	online := oracle.online
	for _, c := range cs {
		if !c.down {
			online = max(online, c.online)
		}
	}
	time.Sleep(time.Until(time.Unix(online+30, 0)))
	for _, c := range cs {
		if c.down {
			continue
		}
		t.Run(c.form+"/data", func(t *testing.T) {
			for _, chart := range chainDataCharts {
				last := hopLast(t, c.topology, chainChild.Hostname, chainHops, chart) - 1
				live := hopSeries(t, c.topology, chainChild.Hostname, chainHops, chart, c.online+5, last)
				for j, pair := range [][2]int{{0, 1}, {1, 2}} {
					if live[pair[0]] != live[pair[1]] {
						t.Errorf("%s live, hop %d: %s", chart, j+1,
							firstDifference([]byte(live[pair[0]]), []byte(live[pair[1]])))
					}
				}
				if _, rows := csvRows(live[0]); len(rows) < 20 {
					t.Errorf("%s live: %d rows", chart, len(rows))
				}
				first := jsonNumber(t, c.d("c"), "/api/v1/chart?chart="+chart, "first_entry")
				whole := hopSeries(t, c.topology, chainChild.Hostname, chainHops, chart, first, last)
				compareHops(t, chart+" full", whole)
			}
		})
	}

	// P4b: one call from the grandparent to the child's plugin through the proxy
	chainFn(t, cs)

	// P5: the child leaves; the proxy and the grandparent keep its stale path
	forEach(cs, func(c *chain) {
		c.stopped = time.Now().Unix()
		if err := c.d("c").Stop(); err != nil {
			t.Errorf("%s: stop the child: %v", c.form, err)
		}
		if _, b, ok := waitHop(c.addr("gp"), chainChild.MachineGUID, false, 30*time.Second); !ok {
			t.Errorf("%s: the grandparent kept the child as a child: %.300s", c.form, b)
		}
	})
	time.Sleep(3 * time.Second)
	if len(parentRecords(t, oracle.d("p"), "RECEIVER LEFT", nil)) == 0 {
		t.Fatalf("the oracle proxy wrote no RECEIVER LEFT record")
	}
	// rrdhost_set_receiver()'s connections++ (stream-receiver.c:1407): a host a receiver attached to and left is
	// offline, not archived (rrdhost-status.c:186-190); the oracle must say so at both hops
	var leftInfo [2][]byte
	for i, at := range []string{"gp", "p"} {
		b, err := streamInfoMasked(oracle.addr(at), chainChild.MachineGUID)
		if err != nil || !bytes.Contains(b, []byte(`"ingest_type":"archived"`)) ||
			!bytes.Contains(b, []byte(`"ingest_status":"offline"`)) {
			t.Fatalf("the oracle %s's stream_info of the child after the leave: %v %.300s", at, err, b)
		}
		leftInfo[i] = b
	}
	for _, c := range cs[1:] {
		if !c.down {
			t.Run(c.form+"/leave", func(t *testing.T) {
				topoViews(t, "leave", oracle.topology, c.topology, [][2]string{gpChildView, proxyChildView}, entryTimes)
				for i, at := range []string{"gp", "p"} {
					b, err := streamInfoMasked(c.addr(at), chainChild.MachineGUID)
					if err != nil || !bytes.Equal(leftInfo[i], b) {
						t.Errorf("%s stream_info of the child after the leave: %v %s", at, err,
							firstDifference(leftInfo[i], b))
					}
				}
			})
		}
	}

	// P6: the child returns; its gap and the blocks the proxy lost at RECEIVER LEFT come through both hops
	forEach(cs, func(c *chain) {
		time.Sleep(time.Until(time.Unix(c.stopped+15, 0)))
		g.wait()
		c.returned = time.Now().Unix()
		if err := c.d("c").Restart(); err != nil {
			t.Errorf("%s: restart the child: %v", c.form, err)
			c.down = true
			return
		}
		if _, b, ok := waitHop(c.addr("gp"), chainChild.MachineGUID, true, 120*time.Second); !ok {
			t.Errorf("%s: the grandparent never had the returned child online: %.300s", c.form, b)
			c.down = true
			return
		}
		if hops, ok := waitPathHops(c.addr("c"), "", full, 30*time.Second); !ok {
			t.Errorf("%s: the returned child's own path has hops %v", c.form, hops)
		}
	})
	if oracle.down {
		t.FailNow()
	}
	for _, c := range cs {
		if c.down {
			continue
		}
		t.Run(c.form+"/return", func(t *testing.T) {
			waitHopsReach(t, c.topology, chainChild.Hostname, chainHops, chainDataCharts, c.returned+10, 30*time.Second)
			for _, chart := range chainDataCharts {
				s := hopSeries(t, c.topology, chainChild.Hostname, chainHops, chart, c.stopped-20, c.returned+10)
				if c == oracle {
					// newest first: values after the return, one run of nulls (the gap), values before the stop
					_, rows := csvRows(s[0])
					after, nulls, before, runs := 0, 0, 0, 0
					for k, r := range rows {
						switch {
						case nullRow(r):
							nulls++
							if k == 0 || !nullRow(rows[k-1]) {
								runs++
							}
						case nulls == 0:
							after++
						default:
							before++
						}
					}
					if after < 1 || nulls < 10 || runs != 1 || before < 15 {
						t.Errorf("%s: the oracle child's own series: %d rows after the return, %d null rows in %d runs, "+
							"%d rows before the stop", chart, after, nulls, runs, before)
					}
				}
				compareHops(t, chart, s)
			}
			if c != oracle {
				topoViews(t, "return", oracle.topology, c.topology, chainPathViews, entryRestartTimes)
			}
		})
	}

	// P7: the records of both sessions and both leaves, as sets
	forEach(cs, func(c *chain) {
		if err := c.d("c").Stop(); err != nil {
			t.Errorf("%s: stop the child: %v", c.form, err)
		}
		if _, b, ok := waitHop(c.addr("gp"), chainChild.MachineGUID, false, 30*time.Second); !ok {
			t.Errorf("%s: the grandparent kept the child as a child: %.300s", c.form, b)
		}
	})
	time.Sleep(3 * time.Second)
	want := chainRecords(t, oracle)
	for _, set := range []string{"proxy receiver", "grandparent receiver"} {
		for _, form := range []string{"connected and ready to receive data, new node NEVER CONNECTED",
			"connected and ready to receive data, last sample in the db D ago NEVER CONNECTED"} {
			if !slices.ContainsFunc(want[set], func(l string) bool { return strings.Contains(l, form) }) {
				t.Errorf("the oracle's %s records have no %q", set, form)
			}
		}
	}
	for _, name := range slices.Sorted(maps.Keys(want)) {
		t.Logf("oracle %s records:\n%s", name, strings.Join(want[name], "\n"))
	}
	for _, c := range cs[1:] {
		if c.down {
			continue
		}
		t.Run(c.form+"/records", func(t *testing.T) { compareRecordSets(t, want, chainRecords(t, c)) })
	}
}
