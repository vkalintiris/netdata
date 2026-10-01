// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"errors"
	"maps"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// childArchived is the record of the proxy's cleanup archiving the orphaned child (DEBUG).
const childArchived = "RRD: 'host:parity-chain-child' is now in archive mode"

var (
	// a sender whose parent was killed: the socket's error, its hang-up or its EOF, as the kill's FIN or RST falls
	killSendErrRe = regexp.MustCompile(`(socket reports errors|connection closed by remote end \(HUP\)|` +
		`socket reports EOF \(closed by parent\)) restarting connection`)
	// a receiver whose peer was killed: FIN, RST or the probe's form (normalizeLog wrote its fd as `fd N`)
	killRcvCloseRe = regexp.MustCompile(`(\]: )([A-Z ]+?( fd N - closing receiver connection\.| - closing connection;[^"]*)|socket closed by remote - closing connection)`)
	// the socket reasons of those forms
	killReasonRe = regexp.MustCompile(`reason=\\"(DISCONNECTED SOCKET (CLOSED BY REMOTE END|READ FAILED|WRITE FAILED)|` +
		`DISCONNECT SOCKET ERROR)\\"`)
	errnoFieldRe = regexp.MustCompile(` errno="[^"]*"`)
	// the gate records' block (not the web thread's STREAM URL, which starts with a slash)
	requestFieldRe = regexp.MustCompile(` request="[^/"][^"]*"`)
	// the refused probes of a parent that is down, in whichever order the chain's hosts try it
	refusedHostRe = regexp.MustCompile(`'parity-chain-(child|proxy|gp)'`)
	// a down or reviving parent's probes: which of the proxy's senders meets the refusal and which the 404 is timing
	downProbeRe = regexp.MustCompile(`Failed to connect to '|failed to connect for stream info|failed to extract fields from JSON stream info`)
	probeNodeRe = regexp.MustCompile(` node=parity-chain-(child|proxy) `)
	// what a kill may add or not, by timing: a write racing the close, a revived parent still loading its hosts or
	// closing a stream info probe it accepted while starting
	killRaceRe = regexp.MustCompile(`socket reports error while writing|remote server is initializing|` +
		`host is initializing, retry later|socket receive error while querying stream info`)
	// entryKillTimes masks a path entry's times and its start-time median, which a restart moves; the shutdown-time
	// median stays compared (0 after a SIGKILL)
	entryKillTimes = regexp.MustCompile(`("since":)\d+(,\s*"first_time_t":)\d+,\s*"start_time":\d+`)
)

// relaunch boots a stopped or killed daemon again on its run directory and port, trying again while another
// process holds the port.
func relaunch(d *daemon.Daemon) error {
	var err error
	for range 5 {
		if err = d.Restart(); err == nil || !errors.Is(err, daemon.ErrPortTaken) {
			return err
		}
		time.Sleep(3 * time.Second)
	}
	return err
}

// lossRecords normalizes the records of a chain that lost an agent or a child: the gate records' block and chart
// fields (the block the parser was reading when the gate flipped) and a retry's time; as a sorted set, without the
// debug lines.
func lossRecords(lines []string) []string {
	var out []string
	for _, l := range lines {
		if strings.Contains(l, "level=debug") {
			continue
		}
		l = strings.Join(strings.Fields(requestFieldRe.ReplaceAllString(chartFieldsRe.ReplaceAllString(l, ""), "")), " ")
		out = append(out, retryAtRe.ReplaceAllString(l, `will retry in ${1} secs, at T"`))
	}
	slices.Sort(out)
	return slices.Compact(out)
}

// killRecords is lossRecords for a kill: the disconnects' forms, socket reasons and errno, the refused probes'
// hosts, and without the lines a kill adds or not by timing.
func killRecords(lines []string) []string {
	var out []string
	for _, l := range lines {
		if killRaceRe.MatchString(l) {
			continue
		}
		if killSendErrRe.MatchString(l) || killRcvCloseRe.MatchString(l) {
			// the errno of a close follows the FIN or RST that met it
			l = errnoFieldRe.ReplaceAllString(l, "")
		}
		l = killSendErrRe.ReplaceAllString(l, "PEER GONE restarting connection")
		l = killRcvCloseRe.ReplaceAllString(l, "${1}PEER GONE")
		l = killReasonRe.ReplaceAllString(l, `reason=\"R\"`)
		if downProbeRe.MatchString(l) {
			l = probeNodeRe.ReplaceAllString(l, " node=H ")
		}
		out = append(out, refusedHostRe.ReplaceAllString(l, "'H'"))
	}
	return lossRecords(out)
}

// chainLossRecords are chainRecords plus the proxy's other records (its sender's among them) and the loads of
// archived hosts at the proxy and the grandparent, each normalized by `set`.
func chainLossRecords(t *testing.T, c *chain, set func([]string) []string) map[string][]string {
	t.Helper()
	recs := chainRecords(t, c)
	var sender []string
	for _, l := range rchildRecords(t, c.d("p")) {
		if !strings.Contains(l, "STREAM RCV") {
			sender = append(sender, l)
		}
	}
	recs["proxy sender"] = sender
	for _, at := range []string{"p", "gp"} {
		var archived []string
		for _, l := range logLines(t, c.d(at).Opts.RunDir, "daemon.log") {
			if strings.Contains(l, "archived hosts") || strings.Contains(l, "streaming disabled (to ''") {
				archived = append(archived, normalizeLog(l, c.d(at).Opts.RunDir, ""))
			}
		}
		recs[at+" archived"] = archived
	}
	for k, v := range recs {
		recs[k] = set(v)
	}
	return recs
}

// entryTimesOf are the start and shutdown medians of a host's entry in a path answer.
func entryTimesOf(body []byte, hostname string) (start, shutdown int64, ok bool) {
	var v struct {
		Nodes []struct {
			Path []struct {
				Hostname string `json:"hostname"`
				Start    int64  `json:"start_time"`
				Shutdown int64  `json:"shutdown_time"`
			} `json:"streaming_path"`
		} `json:"nodes"`
	}
	if json.Unmarshal(body, &v) != nil {
		return 0, 0, false
	}
	for _, n := range v.Nodes {
		for _, e := range n.Path {
			if e.Hostname == hostname {
				return e.Start, e.Shutdown, true
			}
		}
	}
	return 0, 0, false
}

// compareRecordSets compares a candidate chain's record sets with the oracle's.
func compareRecordSets(t *testing.T, want, got map[string][]string) {
	t.Helper()
	for _, name := range slices.Sorted(maps.Keys(want)) {
		diffLines(t, name, want[name], got[name])
	}
}

// TestStreamChainLong (check `stream.chain-long`, PARITY_LONG; milestone 7 commit 8i, D121.1, D121.5; plan
// `evidence/2026-09-30-plan-m7-8i-4-chain-long.md`): the chains of `stream.chain` losing an agent. `kill-proxy` and
// `kill-gp` SIGKILL the proxy or the grandparent of every chain at once and restart it on its run directory 20 s
// later; `orphan` keeps the child away until the proxy cleans its host up (HOST CLEANUP frees the proxied sender),
// then brings it back (the revival sets the sender up again). Each compares the candidate chains with c-c-c during and
// after, the hops' data within each chain over the loss, and the records as sets.
func TestStreamChainLong(t *testing.T) {
	if os.Getenv("PARITY_LONG") == "" {
		t.Skip("PARITY_LONG unset (each case runs for minutes)")
	}
	t.Run("kill-proxy", func(t *testing.T) { chainKill(t, "p") })
	t.Run("kill-gp", func(t *testing.T) { chainKill(t, "gp") })
	t.Run("orphan", chainOrphan)
}

// chainKill kills `node` (the proxy or the grandparent) in every chain once each is steady, restarts it 20 s later,
// and compares the chains while it is down, after it is back, and the records.
func chainKill(t *testing.T, node string) {
	g := &stagger{gap: 2 * time.Second}
	cs := startChains(t, g, nil)
	oracle := cs[0]
	steady := oracle.online
	for _, c := range cs {
		if !c.down {
			steady = max(steady, c.online)
		}
	}
	k := steady + 25
	time.Sleep(time.Until(time.Unix(k, 0)))
	forChains(cs, func(c *chain) {
		if err := c.d(node).Kill(); err != nil {
			t.Errorf("%s: kill %s: %v", c.form, node, err)
			c.down = true
		}
	})
	if oracle.down {
		t.FailNow()
	}

	// while it is down: the survivors' views (a socket's disconnect cuts no path)
	time.Sleep(time.Until(time.Unix(k+8, 0)))
	down := [][2]string{gpChildView, gpProxyView, childOwnView}
	if node == "gp" {
		down = [][2]string{proxyChildView, proxyOwnView, childOwnView}
		if hops, ok := waitPathHops(oracle.addr("p"), "&nodes=parity-chain-child", fullPath, 5*time.Second); !ok {
			t.Errorf("the oracle proxy's path of the child while the grandparent is down: %v", hops)
		}
	}
	if hops, ok := waitPathHops(oracle.addr("c"), "", fullPath, 5*time.Second); !ok {
		t.Errorf("the oracle child's own path while the %s is down: %v", node, hops)
	}
	for _, c := range cs[1:] {
		if !c.down {
			t.Run(c.form+"/down", func(t *testing.T) {
				chainViews(t, "down", oracle, c, down, entryTimes)
			})
		}
	}

	// back 20 s after the kill: the child, and the proxy, online again at the grandparent
	time.Sleep(time.Until(time.Unix(k+20, 0)))
	forChains(cs, func(c *chain) {
		g.wait()
		if err := relaunch(c.d(node)); err != nil {
			t.Errorf("%s: relaunch %s: %v", c.form, node, err)
			c.down = true
			return
		}
		deadline := time.Unix(k+150, 0)
		hops := []struct{ at, guid string }{{"gp", chainProxy.MachineGUID}, {"gp", chainChild.MachineGUID}}
		if node == "p" {
			hops = append([]struct{ at, guid string }{{"p", chainChild.MachineGUID}}, hops...)
		}
		for _, h := range hops {
			if _, b, ok := waitHop(c.addr(h.at), h.guid, true, time.Until(deadline)); !ok {
				t.Errorf("%s: %s never had %s online again: %.300s", c.form, h.at, h.guid, b)
				c.down = true
				return
			}
		}
		c.returned = time.Now().Unix()
		t.Logf("%s: back %d s after the kill", c.form, c.returned-k)
		if hops, ok := waitPathHops(c.addr("c"), "", fullPath, 30*time.Second); !ok {
			t.Errorf("%s: the child's own path after the %s's return has hops %v", c.form, node, hops)
		}
	})
	if oracle.down {
		t.FailNow()
	}

	// the data over the loss, within each chain: every hop has what the child collected
	for _, c := range cs {
		if c.down {
			continue
		}
		t.Run(c.form+"/data", func(t *testing.T) {
			for end := time.Now().Add(30 * time.Second); ; time.Sleep(time.Second) {
				settled := true
				for _, chart := range chainDataCharts {
					settled = settled && chainLast(t, c, chart) >= c.returned+10
				}
				if settled {
					break
				}
				if time.Now().After(end) {
					t.Errorf("the hops' data did not reach 10 s after the return within 30 s")
					break
				}
			}
			for _, chart := range chainDataCharts {
				s := chainSeries(t, c, chart, k-20, c.returned+10)
				if _, rows := csvRows(s[0]); len(rows) < 40 || slices.ContainsFunc(rows, nullRow) {
					t.Errorf("%s: the child's own series has %d rows, some null", chart, len(rows))
				}
				for j, pair := range [][2]int{{0, 1}, {1, 2}} {
					tolerated, diff := seriesExtraNulls(s[pair[0]], s[pair[1]], 1)
					if diff != "" {
						t.Errorf("%s, hop %d: %s", chart, j+1, diff)
					}
					if len(tolerated) > 0 {
						t.Logf("%s, hop %d: tolerated %q", chart, j+1, tolerated)
					}
				}
				if node == "gp" {
					// the proxy never stopped: the grandparent has its own series too
					own := seriesOf(t, c.d("p"), "", chart, k-20, c.returned+10)
					up := seriesOf(t, c.d("gp"), "/host/parity-chain-proxy", chart, k-20, c.returned+10)
					if tolerated, diff := seriesExtraNulls(own, up, 1); diff != "" {
						t.Errorf("%s, the proxy's own at the grandparent: %s", chart, diff)
					} else if len(tolerated) > 0 {
						t.Logf("%s, the proxy's own at the grandparent: tolerated %q", chart, tolerated)
					}
				}
			}
		})
	}

	// after: the views (the restarted agent's entry, in every chain: a start median, no clean shutdown, which the
	// views' time mask leaves out), the stream_info
	killed := map[string]string{"p": "parity-chain-proxy", "gp": "parity-chain-gp"}[node]
	for _, c := range cs {
		if c.down {
			continue
		}
		b, err := rawExchange(c.addr("gp"),
			[]byte("GET /api/v3/stream_path?nodes=parity-chain-child&options=minify HTTP/1.1\r\n\r\n"), 5*time.Second)
		if start, shutdown, ok := entryTimesOf(httpBody(b), killed); err != nil || !ok || start == 0 || shutdown != 0 {
			t.Errorf("%s: the grandparent's path: %s's entry start %d shutdown %d (%v)", c.form, killed, start,
				shutdown, err)
		}
	}
	for _, c := range cs[1:] {
		if c.down {
			continue
		}
		t.Run(c.form+"/final", func(t *testing.T) {
			if node == "p" {
				// a restarted proxy sends its labels once its sender is ready, the child back or not yet: the
				// grandparent's copy of its `_is_parent` races
				chainViews(t, "final", oracle, c, [][2]string{gpChildView, proxyChildView, proxyOwnView, childOwnView},
					entryKillTimes)
				chainViews(t, "final", oracle, c, [][2]string{gpProxyView}, entryKillTimes, "_is_parent")
			} else {
				chainViews(t, "final", oracle, c, chainPathViews, entryKillTimes)
			}
			for _, at := range []string{"gp", "p"} {
				o, oerr := streamInfoMasked(oracle.addr(at), chainChild.MachineGUID)
				got, gerr := streamInfoMasked(c.addr(at), chainChild.MachineGUID)
				if oerr != nil || gerr != nil || string(o) != string(got) {
					t.Errorf("%s stream_info of the child: %v %v %s", at, oerr, gerr, firstDifference(o, got))
				}
			}
		})
	}

	// the records of the loss and both sessions, as sets
	forChains(cs, func(c *chain) {
		if err := c.d("c").Stop(); err != nil {
			t.Errorf("%s: stop the child: %v", c.form, err)
		}
		if _, b, ok := waitHop(c.addr("gp"), chainChild.MachineGUID, false, 30*time.Second); !ok {
			t.Errorf("%s: the grandparent kept the child as a child: %.300s", c.form, b)
		}
	})
	time.Sleep(3 * time.Second)
	want := chainLossRecords(t, oracle, killRecords)
	for _, name := range slices.Sorted(maps.Keys(want)) {
		t.Logf("oracle %s records:\n%s", name, strings.Join(want[name], "\n"))
	}
	for _, c := range cs[1:] {
		if !c.down {
			t.Run(c.form+"/records", func(t *testing.T) { compareRecordSets(t, want, chainLossRecords(t, c, killRecords)) })
		}
	}
}

// chainOrphan keeps the child away until the proxy cleans its host up (orphaned after 90 s; with health run every
// second the cleanup's iteration condition holds in time), then brings it back.
func chainOrphan(t *testing.T) {
	g := &stagger{gap: 2 * time.Second}
	cs := startChains(t, g, func(rows []topoNode) {
		for i := range rows {
			if rows[i].name == "p" {
				rows[i].opts.DBExtra = "    cleanup orphan hosts after = 90s\n"
				rows[i].opts.HealthExtra = "    run at least every = 1s\n"
				// the archive's record, the cleanup's only trace, is DEBUG
				rows[i].opts.LogsExtra = "    level = debug\n"
			}
		}
	})
	oracle := cs[0]
	steady := oracle.online
	for _, c := range cs {
		if !c.down {
			steady = max(steady, c.online)
		}
	}
	time.Sleep(time.Until(time.Unix(steady+25, 0)))
	forChains(cs, func(c *chain) {
		c.stopped = time.Now().Unix()
		if err := c.d("c").Stop(); err != nil {
			t.Errorf("%s: stop the child: %v", c.form, err)
		}
		if _, b, ok := waitHop(c.addr("gp"), chainChild.MachineGUID, false, 30*time.Second); !ok {
			t.Errorf("%s: the grandparent kept the child as a child: %.300s", c.form, b)
		}
	})

	// the proxy cleans the orphan up: its maintenance archives the host and frees its sender (every ~12-14 s in C, ~11-12
	// s in Rust, once eligible); nothing an API shows changes then (the path, the charts and stream_info changed at
	// the leave), so the wait reads the archive's record
	forChains(cs, func(c *chain) {
		log := filepath.Join(c.d("p").Opts.RunDir, "log", "daemon.log")
		for end := time.Unix(c.stopped+150, 0); ; time.Sleep(time.Second) {
			if b, err := os.ReadFile(log); err == nil && bytes.Contains(b, []byte(childArchived)) {
				after := time.Now().Unix() - c.stopped
				t.Logf("%s: the proxy archived the child %d s after it left", c.form, after)
				if after < 90 {
					t.Errorf("%s: the proxy archived the child %d s after it left, before the orphan time", c.form, after)
				}
				return
			}
			if time.Now().After(end) {
				t.Errorf("%s: the proxy never archived the child", c.form)
				c.down = true
				return
			}
		}
	})
	if oracle.down {
		t.FailNow()
	}
	cleaned := time.Now().Unix()
	time.Sleep(2 * time.Second)
	for _, c := range cs[1:] {
		if c.down {
			continue
		}
		t.Run(c.form+"/archived", func(t *testing.T) {
			chainViews(t, "archived", oracle, c, [][2]string{gpChildView, proxyChildView}, entryTimes)
			o, oerr := streamInfoMasked(oracle.addr("p"), chainChild.MachineGUID)
			got, gerr := streamInfoMasked(c.addr("p"), chainChild.MachineGUID)
			if oerr != nil || gerr != nil || string(o) != string(got) {
				t.Errorf("the proxy's stream_info of the child: %v %v %s", oerr, gerr, firstDifference(o, got))
			}
		})
	}

	// the child returns: the revival sets the proxy's sender up again
	time.Sleep(time.Until(time.Unix(cleaned+5, 0)))
	forChains(cs, func(c *chain) {
		g.wait()
		c.returned = time.Now().Unix()
		if err := c.d("c").Restart(); err != nil {
			t.Errorf("%s: restart the child: %v", c.form, err)
			c.down = true
			return
		}
		for _, at := range []string{"p", "gp"} {
			if _, b, ok := waitHop(c.addr(at), chainChild.MachineGUID, true, 120*time.Second); !ok {
				t.Errorf("%s: %s never had the returned child online: %.300s", c.form, at, b)
				c.down = true
				return
			}
		}
		if hops, ok := waitPathHops(c.addr("c"), "", fullPath, 30*time.Second); !ok {
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
			for end := time.Now().Add(30 * time.Second); ; time.Sleep(time.Second) {
				settled := true
				for _, chart := range chainDataCharts {
					settled = settled && chainLast(t, c, chart) >= c.returned+10
				}
				if settled {
					break
				}
				if time.Now().After(end) {
					t.Errorf("the hops' data did not reach 10 s after the return within 30 s")
					break
				}
			}
			for _, chart := range chainDataCharts {
				s := chainSeries(t, c, chart, c.stopped-20, c.returned+10)
				// the premise: the child's own series has the gap, and values around it
				if _, rows := csvRows(s[0]); slices.ContainsFunc(rows, nullRow) {
					nulls := 0
					for _, r := range rows {
						if nullRow(r) {
							nulls++
						}
					}
					if nulls < 85 || len(rows)-nulls < 15 {
						t.Errorf("%s: the child's own series has %d null rows of %d", chart, nulls, len(rows))
					}
				} else {
					t.Errorf("%s: the child's own series has no gap in %d rows", chart, len(rows))
				}
				for j, pair := range [][2]int{{0, 1}, {1, 2}} {
					tolerated, diff := seriesExtraNulls(s[pair[0]], s[pair[1]], 1)
					if diff != "" {
						t.Errorf("%s, hop %d: %s", chart, j+1, diff)
					}
					if len(tolerated) > 0 {
						t.Logf("%s, hop %d: tolerated %q", chart, j+1, tolerated)
					}
				}
			}
			if c != oracle {
				chainViews(t, "return", oracle, c, chainPathViews, entryRestartTimes)
			}
		})
	}

	// the records, as sets
	forChains(cs, func(c *chain) {
		if err := c.d("c").Stop(); err != nil {
			t.Errorf("%s: stop the child: %v", c.form, err)
		}
		if _, b, ok := waitHop(c.addr("gp"), chainChild.MachineGUID, false, 30*time.Second); !ok {
			t.Errorf("%s: the grandparent kept the child as a child: %.300s", c.form, b)
		}
	})
	time.Sleep(3 * time.Second)
	// nothing was killed: the loss's masks only, and the archive's record
	records := func(c *chain) map[string][]string {
		recs := chainLossRecords(t, c, lossRecords)
		for _, l := range logLines(t, c.d("p").Opts.RunDir, "daemon.log") {
			if strings.Contains(l, childArchived) {
				recs["proxy archive"] = append(recs["proxy archive"], normalizeLog(l, c.d("p").Opts.RunDir, ""))
			}
		}
		return recs
	}
	want := records(oracle)
	for _, name := range slices.Sorted(maps.Keys(want)) {
		t.Logf("oracle %s records:\n%s", name, strings.Join(want[name], "\n"))
	}
	for _, c := range cs[1:] {
		if !c.down {
			t.Run(c.form+"/records", func(t *testing.T) { compareRecordSets(t, want, records(c)) })
		}
	}
}
