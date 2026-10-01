// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"os"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// stallRecordRe are the stall check's records and the disconnect it causes.
var stallRecordRe = regexp.MustCompile(`REPLICATION STALLED|REPLICATION EXCEPTIONS SUMMARY`)

// TestReplicationStall (check `stream.rchild-replication/stall`, PARITY_LONG; milestone 7 commit 6, D105.9; map
// `knowledge/map-m7-commit6-replication.md` §14.3): a C child and a Rust child, in parallel at `level = debug`, each
// stream to a scripted parent that asks only one chart's replication (`"true" 0 0`) and nothing for the others. Their
// replication stops moving, and the stream thread's ten-minute check disconnects them (the first check sees the
// counters move, the second finds them still): each lists its unfinished charts, logs C's exceptions summary with the
// one request received and answered, disconnects with REPLICATION STALLED and connects again. Compared: those records,
// as sets.
func TestReplicationStall(t *testing.T) {
	if os.Getenv("PARITY_LONG") != "1" {
		t.Skip("PARITY_LONG=1 runs the replication stall check (about 25 minutes)")
	}
	const asked = "netdata.uptime"
	var records [2][]string
	runBoth(t, func(i int, bin string, role Role) {
		parent, err := stream.StartParent(func(r stream.Request) stream.Answer {
			a := stream.PlaintextAnswer(r)
			a.Replay = func(ev stream.ReplayEvent) []string {
				if !ev.Answer && ev.Chart == asked {
					return []string{`REPLAY_CHART "` + asked + `" "true" 0 0`}
				}
				return nil
			}
			return a
		})
		if err != nil {
			t.Error(err)
			return
		}
		t.Cleanup(func() { parent.Close() })
		id := daemon.Identity{Hostname: "stall-child", StreamKey: "5a1e0000-0000-4000-8000-0000000000c5",
			MachineGUID: "5a1e0000-0000-4000-8000-0000000000c6"}
		d, err := daemon.Start(daemon.Options{Binary: bin, RunDir: runDir(t, Role(string(role)+"-stall")),
			Identity: &id, DBMode: "alloc", StorageTiers: 1, NoStreamKey: true, LogsExtra: "    level = debug\n",
			StreamTo: &daemon.StreamTo{Destination: parent.Addr(), APIKey: parentIdentity.StreamKey,
				Extra: "    reconnect delay = 5\n"}})
		if err != nil {
			t.Errorf("start %s: %v", role, err)
			return
		}
		t.Cleanup(func() { _ = d.Stop() })
		if parent.WaitSession(1, 60*time.Second) == nil {
			t.Errorf("%s: no session within 60 s", role)
			return
		}
		deadline := time.Now().Add(25 * time.Minute)
		for !logContains(t, d, "REPLICATION EXCEPTIONS SUMMARY") {
			if time.Now().After(deadline) {
				t.Errorf("%s: no stall within 25 minutes", role)
				return
			}
			time.Sleep(5 * time.Second)
		}
		if parent.WaitSession(2, 60*time.Second) == nil {
			t.Errorf("%s: no second session within 60 s of the stall", role)
		}
		_ = d.Stop()
		for _, r := range rchildRecords(t, d) {
			if stallRecordRe.MatchString(r) {
				records[i] = append(records[i], r)
			}
		}
	})
	if len(records[0]) == 0 {
		t.Fatal("the oracle logged no stall records")
	}
	diffLines(t, "the stall records", records[0], records[1])
	t.Logf("records:\n%s", strings.Join(records[0], "\n"))
}

// logContains tells whether a daemon's log has a record containing `text`.
func logContains(t *testing.T, d *daemon.Daemon, text string) bool {
	t.Helper()
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if strings.Contains(l, text) {
			return true
		}
	}
	return false
}

// rstallHost is the raw child of `stream.receiver-stall`.
var rstallHost = stream.HostInfo{Hostname: "parity-rstall", MachineGUID: "5a1e0000-0000-4000-8000-00000000c5a1"}

// rstallRecordRe are the receiver's stall records and its disconnect.
var rstallRecordRe = regexp.MustCompile(`REPLICATION EXCEPTIONS|reason=\\"REPLICATION STALLED\\"|REPLICATION STALLED`)

// TestReceiverReplicationStall (check `stream.receiver-stall`, PARITY_LONG; M7 10f, D122.11): a raw child with
// REPLICATION defines two charts in each parent of a pair (`level = debug`) and answers the request of one; the
// other's is never answered, so it stays in progress while nothing is pending (backfill_pending counts only requests
// not sent yet). The stream thread's ten-minute check sees the counters move at its first pass and still at its
// second, about 20 minutes after the connection (no key shortens it): it lists the unfinished chart, logs the summary
// (2 requested, 3 replies: 2 definitions and one REND), disconnects with REPLICATION STALLED and closes the socket.
// Compared: those records, as sets, and the child seeing the close.
func TestReceiverReplicationStall(t *testing.T) {
	if os.Getenv("PARITY_LONG") != "1" {
		t.Skip("PARITY_LONG=1 runs the receiver's replication stall check (about 21 minutes)")
	}
	t.Parallel()
	p := StartPair(t, daemon.Options{StorageTiers: 1, StreamMemoryMode: "ram", PulseOff: true,
		LogsExtra: "    level = debug\n"}, parentIdentity)
	var conns [2]*stream.Conn
	for i, side := range p.Each() {
		c, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, rstallHost, stream.CapsReplication)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		t.Cleanup(func() { _ = c.Close() })
		now := time.Now().Unix()
		for _, id := range []string{"rstall.open", "rstall.done"} {
			c.Linef("CHART '%s' '' 't' 'u' 'f' '%s' line 1000 1 '' rstall corpus", id, id)
			c.Linef("DIMENSION 'd' '' absolute 1 1 ''")
			c.ChartDefinitionEnd(0, 0, now)
		}
		if err := c.Flush(); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		// answer rstall.done's request as a C child with nothing to replicate does
		deadline := time.Now().Add(30 * time.Second)
		for {
			l, err := c.ReadLine(deadline)
			if err != nil {
				t.Fatalf("%s: no request for rstall.done: %v", side.Role, err)
			}
			if strings.HasPrefix(l, `REPLAY_CHART "rstall.done"`) {
				break
			}
		}
		c.Linef("RBEGIN 'rstall.done'")
		c.Linef("REND 1 0 0 true 0 0 %d", time.Now().Unix())
		if err := c.Flush(); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		conns[i] = c
	}
	deadline := time.Now().Add(25 * time.Minute)
	for _, side := range p.Each() {
		for !logContains(t, side.Daemon, "REPLICATION EXCEPTIONS SUMMARY") {
			if time.Now().After(deadline) {
				t.Fatalf("%s: no stall within 25 minutes", side.Role)
			}
			time.Sleep(5 * time.Second)
		}
	}
	var closed [2]bool
	for i, c := range conns {
		for {
			_, err := c.ReadLine(time.Now().Add(10 * time.Second))
			if err != nil {
				closed[i] = !strings.Contains(err.Error(), "timeout")
				break
			}
		}
	}
	if !closed[0] || closed[1] != closed[0] {
		t.Errorf("the child saw the close: oracle %v, candidate %v", closed[0], closed[1])
	}
	time.Sleep(2 * time.Second)
	var recs [2][]string
	for i, side := range p.Each() {
		for _, l := range parentRecords(t, side.Daemon, "STREAM RCV", nil) {
			if rstallRecordRe.MatchString(l) {
				// C's first record after the stream thread's last non-blocking read carries that read's EAGAIN
				// (nd_log() takes errno, then clears it: nd_log.c:341, :410): stale, ignored as D36's
				recs[i] = append(recs[i], errnoRe.ReplaceAllString(l, ""))
			}
		}
	}
	want := "node has 1 stalled replication requests (1 finished). We have requested 2 and got replies for 3"
	if !strings.Contains(strings.Join(recs[0], "\n"), want) {
		t.Errorf("the oracle's summary is not %q:\n%s", want, strings.Join(recs[0], "\n"))
	}
	diffLines(t, "the receiver's stall records", recs[0], recs[1])
	t.Logf("records:\n%s", strings.Join(recs[0], "\n"))
}
