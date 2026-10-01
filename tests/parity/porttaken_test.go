// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"errors"
	"net"
	"testing"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// TestPortTakenIsReported (harness self-check, review R49 S1): an agent started on a preset port another process
// holds fails with daemon.ErrPortTaken, read from its own bind failure in the daemon log, so a topology picks its
// ports again and a relaunch retries.
func TestPortTakenIsReported(t *testing.T) {
	bins := binaries(t)
	id := daemon.Identity{Hostname: "parity-port-taken", StreamKey: "5a1e0000-0000-4000-8000-00000000c602",
		MachineGUID: "5a1e0000-0000-4000-8000-00000000c601"}
	for i, role := range []Role{Oracle, Candidate} {
		t.Run(string(role), func(t *testing.T) {
			l, err := net.Listen("tcp", "127.0.0.1:0")
			if err != nil {
				t.Fatal(err)
			}
			defer l.Close()
			d, err := daemon.Start(daemon.Options{Binary: bins[i], Port: l.Addr().(*net.TCPAddr).Port,
				RunDir: runDir(t, Role("port-taken-"+string(role))), Identity: &id, StorageTiers: 1})
			if err == nil {
				_ = d.Stop()
				t.Fatal("started on a taken port")
			}
			if !errors.Is(err, daemon.ErrPortTaken) {
				t.Fatalf("Start() = %v, want daemon.ErrPortTaken", err)
			}
		})
	}
}
