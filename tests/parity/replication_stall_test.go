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
