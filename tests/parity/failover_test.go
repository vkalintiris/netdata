// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// failoverChildGUID is the handshake child's machine GUID (handshakeChild), which a probe answer names.
const failoverChildGUID = "5a1e0000-0000-4000-8000-0000000000c4"

// failoverCase is how ACTIVE lets its child go: from the start (its setup), or once the child streams to it (fail).
type failoverCase struct {
	setup func(active *stream.Parent)
	// fail runs when the child's session with ACTIVE started
	fail func(active *stream.Parent, session *stream.Session)
	// failed tells when ACTIVE failed the child, for the setup cases: the first STREAM request or probe it got
	failed func(active *stream.Parent) bool
	// within is how soon after the failure the STANDBY session must start
	within time.Duration
	// want are records each side's connector must have logged, by stub name, before the records are compared
	want map[string]string
	// logs is the child's [logs] section, beyond the harness's
	logs string
	// sender: the child's stream thread's records compare too (its sender's, with the parent's address)
	sender bool
}

var failoverCases = map[string]failoverCase{
	// ACTIVE hangs up mid-session: the child goes to STANDBY at once, without trying ACTIVE again (the disconnect
	// postpones it)
	"disconnect": {
		fail:   func(_ *stream.Parent, s *stream.Session) { _ = s.Close() },
		within: 3 * time.Second,
	},
	// ACTIVE resets the session (an RST): the child goes to STANDBY at once, and its sender's records read the reset
	// parent's address as unknown, from the socket when written (review R50 m1, D125)
	"reset": {
		fail:   func(_ *stream.Parent, s *stream.Session) { _ = s.Reset() },
		within: 3 * time.Second,
		sender: true,
	},
	// `disconnect` at `level = debug`: the connector's debug records compare too, so a pass that tried the postponed
	// ACTIVE again would show its "connecting to" record (the map's §7.5 mutant 2)
	"debug": {
		fail:   func(_ *stream.Parent, s *stream.Session) { _ = s.Close() },
		within: 3 * time.Second,
		logs:   "    level = debug\n",
	},
	// ACTIVE, ranked first, answers its STREAM requests busy: the same pass moves on to STANDBY
	"busy": {
		setup:  rejecting(stream.RejectBusy),
		failed: func(active *stream.Parent) bool { return len(active.Sessions()) > 0 },
		within: 3 * time.Second,
		want:   map[string]string{"ACTIVE": "remote server is currently busy"},
	},
	// ACTIVE's probe accepts and stays silent past the probe's 5 s receive timeout
	"probe-timeout": {
		setup: func(active *stream.Parent) {
			active.SetProbe(func(string) []byte { time.Sleep(20 * time.Second); return nil })
		},
		failed: func(active *stream.Parent) bool { return len(active.ProbeRequests()) > 0 },
		within: 8 * time.Second,
		want:   map[string]string{"ACTIVE": "timeout"},
	},
}

// TestStreamFailover (check `stream.failover`, D109.3; map `knowledge/map-m7-commit9-topologies.md` §7.1): a child
// has two scripted parents, ACTIVE, which its probe ranks first (it claims the child's data up to 2000 where STANDBY
// answers 404), and STANDBY. ACTIVE fails the child as the case says; the child must stream to STANDBY within the
// case's bound of the failure, without a second session with ACTIVE. Compared between a C child and a Rust child: the
// connector's records as sets (stubs by name) and the request STANDBY received. The case `rust-parents` has a C
// child's real parents fail over instead (testFailoverParents).
func TestStreamFailover(t *testing.T) {
	t.Run("rust-parents", testFailoverParents)
	t.Run("real", testFailoverReal)
	for name, tc := range failoverCases {
		t.Run(name, func(t *testing.T) {
			var records, requests [2][]string
			runBoth(t, func(i int, bin string, role Role) {
				s := startStubs(t, map[string]func(*stream.Parent){
					"ACTIVE": func(p *stream.Parent) {
						p.Probe = func(string) []byte { return streamInfo(failoverChildGUID, "child", "offline") }
						if tc.setup != nil {
							tc.setup(p)
						}
					},
					"STANDBY": func(p *stream.Parent) {},
				})
				active, standby := s.parents["ACTIVE"], s.parents["STANDBY"]
				d := handshakeChild(t, bin, Role(string(role)+"-failover-"+name), s.destination(), tc.logs, "")
				var failed time.Time
				if tc.fail != nil {
					first := active.WaitSession(1, 60*time.Second)
					if first == nil {
						t.Errorf("%s: no session with ACTIVE within 60 s (STANDBY sessions %d)", role,
							len(standby.Sessions()))
						return
					}
					time.Sleep(2 * time.Second)
					failed = time.Now()
					tc.fail(active, first)
				} else {
					deadline := time.Now().Add(60 * time.Second)
					for !tc.failed(active) && time.Now().Before(deadline) {
						time.Sleep(20 * time.Millisecond)
					}
					failed = time.Now()
				}
				second := standby.WaitSession(1, 60*time.Second)
				took := time.Since(failed)
				if second == nil {
					t.Errorf("%s: no session with STANDBY within 60 s", role)
					return
				}
				if took > tc.within {
					t.Errorf("%s: STANDBY's session started %v after the failure (bound %v)", role, took, tc.within)
				}
				if n := len(active.Sessions()); tc.fail != nil && n != 1 {
					t.Errorf("%s: %d sessions with ACTIVE", role, n)
				}
				waitRecords(t, d, s, tc.want, 30*time.Second)
				t.Logf("%s: STANDBY after %v", role, took)
				_ = d.Stop()
				records[i] = handshakeRecords(t, d, s)
				if tc.sender {
					for _, l := range rchildRecords(t, d) {
						if strings.Contains(l, "thread=STREAM[") {
							records[i] = append(records[i], l)
						}
					}
				}
				requests[i] = requestLines(second.Request)
			})
			diffLines(t, "records", records[0], records[1])
			diffLines(t, "STANDBY's request", requests[0], requests[1])
			joined := strings.Join(requests[0], "\n")
			if !strings.Contains(joined, "hops=1") {
				t.Errorf("STANDBY's request has no hops=1: %v", requests[0])
			}
			// the oracle's system-info script ran (an empty one would compare equal between two C children)
			if !strings.Contains(joined, "NETDATA_SYSTEM_KERNEL_NAME=Linux") {
				t.Errorf("the oracle's request has no system info: %v", requests[0])
			}
			if tc.sender && !slices.ContainsFunc(records[0], func(l string) bool {
				return strings.Contains(l, "thread=STREAM[") && strings.Contains(l, "dst_ip=unknown")
			}) {
				t.Errorf("the oracle's sender records never read the reset parent's address as unknown")
			}
			t.Logf("records:\n%s", strings.Join(records[0], "\n"))
		})
	}
}

// failoverParents are the identities of the case `rust-parents`' two parents, which accept the same key.
var failoverParents = [2]daemon.Identity{
	{Hostname: "parity-parent-a", StreamKey: parentIdentity.StreamKey, MachineGUID: "5a1e0000-0000-4000-8000-0000000000a1"},
	{Hostname: "parity-parent-b", StreamKey: parentIdentity.StreamKey, MachineGUID: "5a1e0000-0000-4000-8000-0000000000b1"},
}

// failoverSide is one side of the case `rust-parents`: its two parents, its child, which parent got the child first
// and when it was killed.
type failoverSide struct {
	label   string
	parents [2]*daemon.Daemon
	child   *daemon.Daemon
	holder  int
	killed  int64
	// rewrite names each parent by its part: HOLDER, the killed one, and NEW
	rewrite *strings.Replacer
}

func (s *failoverSide) successor() *daemon.Daemon { return s.parents[1-s.holder] }

// testFailoverParents (case `rust-parents` of `stream.failover`, D109.3): a C child streams to two real parents
// (`ram`, one tier), both C on one side and both Rust on the other. The parent that got the child is killed; the other
// must have the child's receiver within 5 s. Within a side, the new parent's series of the child equal the child's
// own from 20 s before the kill to 10 s after it (replication fills what the new parent never saw). Compared between
// the sides, each parent named by its part: the new parent's stream_info and stream path for the child (the killed
// parent stays listed), the child's own path, the new parent's receiver records, and the children's streaming records.
func testFailoverParents(t *testing.T) {
	const hostname, guid = "parity-failover-child", "5a1e0000-0000-4000-8000-00000000c0cc"
	bins := binaries(t)
	var pairs [2]*Pair
	for j, id := range failoverParents {
		part := "failover-parent-" + string(rune('a'+j))
		pairs[j] = startPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1}, id, bins, [2]string{},
			[2]Role{Role(part + "-oracle"), Role(part + "-candidate")})
	}
	var sides [2]failoverSide
	for i := range sides {
		s := &sides[i]
		for j := range pairs {
			s.parents[j] = pairs[j].Each()[i].Daemon
		}
		s.label = string([2]Role{Oracle, Candidate}[i]) + " parents"
		s.child = startCChild(t, Role("failover-child-"+strconv.Itoa(i)), hostname, guid,
			s.parents[0].Addr+" "+s.parents[1].Addr, false)
	}
	if failOverBoth(t, &sides, hostname, guid) {
		compareFailoverRecords(t, &sides, func(lines []string) []string { return lines })
	}
}

// failoverRealChild is the case `real`'s child: a C child on one side, a Rust child on the other.
var failoverRealChild = daemon.Identity{Hostname: "parity-failover-real", StreamKey: "5a1e0000-0000-4000-8000-00000000fa22",
	MachineGUID: "5a1e0000-0000-4000-8000-00000000fa21"}

// testFailoverReal (case `real` of `stream.failover`, D122.4, milestone 7 commit 9e): a C child on one side and a Rust
// child on the other, each with two real C parents (`ram`, one tier) listed in the same order, which both answer its
// probes 404, so it picks one by chance. The parent that got the child is killed, as in `rust-parents`; 30 s after the
// kill it is relaunched, and for 70 s the child stays with the new parent. Compared between the sides, each parent
// named by its part: `rust-parents`' views and records (a kill's forms as one in the children's), and after the
// restart the child's own path (the killed parent's entry stays, as C cuts a path only at a removal) and the restarted
// parent's stream_info of the child.
func testFailoverReal(t *testing.T) {
	bins := binaries(t)
	g := &stagger{gap: 2 * time.Second}
	var tps [2]*topology
	var errs [2]error
	var wg sync.WaitGroup
	for i, form := range []string{"real-c", "real-r"} {
		wg.Add(1)
		go func() {
			defer wg.Done()
			popts := daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1}
			tps[i], errs[i] = startTopology(t, g, "failover-"+form, bins, []topoNode{
				{name: "p0", impl: 0, id: failoverParents[0], stage: 0, opts: popts},
				{name: "p1", impl: 0, id: failoverParents[1], stage: 0, opts: popts},
				{name: "c", impl: i, id: failoverRealChild, stage: 1, to: []string{"p0", "p1"},
					send: &daemon.StreamTo{Extra: "    reconnect delay = 5\n"},
					opts: daemon.Options{StorageTiers: 1, NoStreamKey: true}},
			}, nil)
		}()
	}
	wg.Wait()
	var sides [2]failoverSide
	for i, err := range errs {
		if err != nil {
			t.Fatalf("side %d: %v", i, err)
		}
		sides[i] = failoverSide{label: [2]string{"C child", "Rust child"}[i],
			parents: [2]*daemon.Daemon{tps[i].d("p0"), tps[i].d("p1")}, child: tps[i].d("c")}
	}
	hostname, guid := failoverRealChild.Hostname, failoverRealChild.MachineGUID
	if !failOverBoth(t, &sides, hostname, guid) {
		return
	}
	b, err := rawExchange(sides[0].child.Addr, []byte("GET /api/v3/stream_path HTTP/1.1\r\n\r\n"), 5*time.Second)
	if err != nil || !strings.Contains(sides[0].rewrite.Replace(string(httpBody(b))), "HOLDER-GUID") {
		t.Errorf("the oracle child's own path does not list the killed parent (%v): %.400s", err, httpBody(b))
	}

	// the killed parent relaunched 30 s after the kill: the child stays with the new one
	for i := range sides {
		wg.Add(1)
		go func() {
			defer wg.Done()
			s := &sides[i]
			time.Sleep(time.Until(time.Unix(s.killed+30, 0)))
			if err := relaunch(s.parents[s.holder]); err != nil {
				t.Errorf("%s: relaunch the killed parent: %v", s.label, err)
				return
			}
			for end := time.Now().Add(70 * time.Second); time.Now().Before(end); time.Sleep(time.Second) {
				back, _ := hasReceiver(s.parents[s.holder].Addr, guid)
				stays, _ := hasReceiver(s.successor().Addr, guid)
				if back || !stays {
					t.Errorf("%s: the child moved after the restart: restarted parent %v, new parent %v", s.label,
						back, stays)
					return
				}
			}
		}()
	}
	wg.Wait()
	rewrites := [2]*strings.Replacer{sides[0].rewrite, sides[1].rewrite}
	compareStreamPathWith(t, "children after the restart", [2]string{sides[0].child.Addr, sides[1].child.Addr},
		rewrites, "/api/v3/stream_path", entryTimes, "_streams_to")
	var info [2][]byte
	for i, s := range sides {
		info[i], err = streamInfoMasked(s.parents[s.holder].Addr, guid)
		if err != nil {
			t.Fatalf("%s: the restarted parent's stream_info: %v", s.label, err)
		}
		info[i] = []byte(s.rewrite.Replace(string(info[i])))
	}
	if !bytes.Equal(info[0], info[1]) {
		t.Errorf("the restarted parents' stream_info of the child: %s", firstDifference(info[0], info[1]))
	}
	compareFailoverRecords(t, &sides, killRecords)
}

// failOverBoth fails both sides over at once (failOver), then compares them, each parent named by its part: the new
// parent's stream_info and path of the child, and the child's own path. It tells whether both failed over.
func failOverBoth(t *testing.T, sides *[2]failoverSide, hostname, guid string) bool {
	var wg sync.WaitGroup
	for i := range sides {
		wg.Add(1)
		go func() {
			defer wg.Done()
			failOver(t, &sides[i], hostname, guid)
		}()
	}
	wg.Wait()
	if t.Failed() {
		return false
	}
	var info [2][]byte
	for i, s := range sides {
		b, err := rawExchange(s.successor().Addr, []byte("GET /api/v3/stream_info?machine_guid="+guid+" HTTP/1.1\r\n\r\n"),
			5*time.Second)
		if err != nil {
			t.Fatal(err)
		}
		info[i] = streamInfoRetentionRe.ReplaceAll([]byte(s.rewrite.Replace(string(httpBody(b)))), []byte(`"${1}":T`))
		info[i] = streamInfoNonceRe.ReplaceAll(info[i], []byte(`"nonce":N`))
	}
	if !bytes.Equal(info[0], info[1]) {
		t.Errorf("the new parents' stream_info differs:\noracle:    %s\ncandidate: %s", info[0], info[1])
	}
	rewrites := [2]*strings.Replacer{sides[0].rewrite, sides[1].rewrite}
	compareStreamPathWith(t, "new parents", [2]string{sides[0].successor().Addr, sides[1].successor().Addr}, rewrites,
		"/api/v3/stream_path", entryTimes, "_streams_to")
	compareStreamPathWith(t, "children", [2]string{sides[0].child.Addr, sides[1].child.Addr}, rewrites,
		"/api/v3/stream_path", entryTimes, "_streams_to")
	return true
}

// compareFailoverRecords stops the children and compares the new parents' receiver records (parts named) and the
// children's records, normalized by `set`.
func compareFailoverRecords(t *testing.T, sides *[2]failoverSide, set func([]string) []string) {
	var received, records [2][]string
	for i, s := range sides {
		_ = s.child.Stop()
		records[i] = set(rchildRecords(t, s.child))
		// ml_capable masked: no ML here (D101.5)
		received[i] = maskRecordSet(parentRecords(t, s.successor(), "STREAM RCV", s.rewrite))
	}
	diffLines(t, "the new parents' receiver records", received[0], received[1])
	diffLines(t, "children's records", records[0], records[1])
	t.Logf("new parent's records:\n%s", strings.Join(received[0], "\n"))
	t.Logf("child's records:\n%s", strings.Join(records[0], "\n"))
}

// failOver runs one side of a real-parents case until its child streams to the second parent with the killed
// window's data replicated.
func failOver(t *testing.T, s *failoverSide, hostname, guid string) {
	name := s.label
	deadline := time.Now().Add(60 * time.Second)
	s.holder = -1
	for s.holder < 0 {
		for j, p := range s.parents {
			if ok, _ := hasReceiver(p.Addr, guid); ok {
				s.holder = j
			}
		}
		if s.holder < 0 && time.Now().After(deadline) {
			t.Errorf("%s: neither parent got the child within 60 s", name)
			return
		}
		time.Sleep(200 * time.Millisecond)
	}
	holder, successor := failoverParents[s.holder], failoverParents[1-s.holder]
	s.rewrite = strings.NewReplacer(holder.MachineGUID, "HOLDER-GUID", holder.Hostname, "HOLDER",
		successor.MachineGUID, "NEW-GUID", successor.Hostname, "NEW")
	if !ingestOnline(s.parents[s.holder].Addr, guid, 60*time.Second) {
		t.Errorf("%s: the child never came online on its first parent", name)
		return
	}
	// the window before the kill that the new parent must replicate
	time.Sleep(20 * time.Second)
	if err := s.parents[s.holder].Kill(); err != nil {
		t.Errorf("%s: kill: %v", name, err)
		return
	}
	killed := time.Now()
	s.killed = killed.Unix()
	for {
		if ok, _ := hasReceiver(s.successor().Addr, guid); ok {
			break
		}
		if time.Since(killed) > 30*time.Second {
			t.Errorf("%s: no receiver on the other parent within 30 s of the kill", name)
			return
		}
		time.Sleep(100 * time.Millisecond)
	}
	took := time.Since(killed)
	if took > 5*time.Second {
		t.Errorf("%s: the other parent had the child %v after the kill (bound 5 s)", name, took)
	}
	t.Logf("%s: the other parent had the child %v after the kill", name, took)
	if !ingestOnline(s.successor().Addr, guid, 60*time.Second) {
		t.Errorf("%s: the child never came online on the new parent", name)
		return
	}
	if wait := time.Until(time.Unix(s.killed+15, 0)); wait > 0 {
		time.Sleep(wait)
	}
	after, before := s.killed-20, s.killed+10
	own := seriesCSV(t, s.child.Addr, "", after, before)
	replicated := seriesCSV(t, s.successor().Addr, "/host/"+hostname, after, before)
	if rows := strings.Count(own, "\n"); rows < 25 {
		t.Errorf("%s: the child's own series has %d rows: %q", name, rows, own)
	} else if own != replicated {
		t.Errorf("%s: the new parent's series of the child differ from the child's own over [%d, %d]:\nchild:  %s\nparent: %s",
			name, after, before, own, replicated)
	}
}

// seriesCSV is `netdata.server_cpu` over [after, before], one point a second, from a daemon's host at `prefix`.
func seriesCSV(t *testing.T, addr, prefix string, after, before int64) string {
	return chartCSV(t, addr, prefix, "netdata.server_cpu", after, before)
}

// chartCSV is a chart over [after, before], one point a second, from a daemon's host at `prefix`.
func chartCSV(t *testing.T, addr, prefix, chart string, after, before int64) string {
	req := fmt.Sprintf("GET %s/api/v1/data?chart=%s&after=%d&before=%d&points=%d&group=average"+
		"&format=csv HTTP/1.1\r\nConnection: close\r\n\r\n", prefix, chart, after, before, before-after+1)
	b, err := rawExchange(addr, []byte(req), 10*time.Second)
	if err != nil {
		t.Errorf("%s%s: %v", addr, prefix, err)
		return ""
	}
	return string(httpBody(b))
}
