// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"strings"
	"testing"
	"time"

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
}

var failoverCases = map[string]failoverCase{
	// ACTIVE hangs up mid-session: the child goes to STANDBY at once, without trying ACTIVE again (the disconnect
	// postpones it)
	"disconnect": {
		fail:   func(_ *stream.Parent, s *stream.Session) { _ = s.Close() },
		within: 3 * time.Second,
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
// connector's records as sets (stubs by name) and the request STANDBY received.
func TestStreamFailover(t *testing.T) {
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
				d := handshakeChild(t, bin, Role(string(role)+"-failover-"+name), s.destination(), "", "")
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
			t.Logf("records:\n%s", strings.Join(records[0], "\n"))
		})
	}
}
