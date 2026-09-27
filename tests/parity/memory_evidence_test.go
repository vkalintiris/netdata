// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// procStatus is a process's `VmHWM` and `VmRSS` (/proc/<pid>/status), as written there.
func procStatus(t *testing.T, pid int) string {
	t.Helper()
	b, err := os.ReadFile(fmt.Sprintf("/proc/%d/status", pid))
	if err != nil {
		t.Fatal(err)
	}
	var out []string
	for _, l := range strings.Split(string(b), "\n") {
		if strings.HasPrefix(l, "VmHWM:") || strings.HasPrefix(l, "VmRSS:") {
			out = append(out, strings.Join(strings.Fields(l), " "))
		}
	}
	return strings.Join(out, ", ")
}

// TestDbengineMemoryEvidence (set PARITY_EVIDENCE=1; evidence, no pass or fail) streams the size workload into both
// daemons, dbengine parents with their pulse charts on, and logs each one's peak and current RSS and its pulse
// memory chart's dbengine caches, before and after the evictors had time to run. GOAL I3's CPU and memory budget is
// not set, so nothing is compared.
func TestDbengineMemoryEvidence(t *testing.T) {
	if os.Getenv("PARITY_EVIDENCE") == "" {
		t.Skip("PARITY_EVIDENCE unset")
	}
	end := time.Now().Unix()
	start := end - r2Span + 1
	p := StartPair(t, daemon.Options{StorageTiers: 1, TierRetentionMB: [3]int{64}}, parentIdentity)
	for _, side := range p.Each() {
		if err := r2gen().stream(side.Daemon, start, end); err != nil {
			t.Fatalf("%s: streaming: %v", side.Role, err)
		}
	}
	for _, side := range p.Each() {
		waitChartsLast(t, side.Daemon, r2child.Hostname, "r2.", r2Charts, end, 10*time.Minute)
	}
	report := func(when string) {
		now := time.Now().Unix()
		for _, side := range p.Each() {
			caches := localData(t, side.Daemon, "netdata.memory&dimensions=dbengine", now-3, now-1, "max")
			t.Logf("%s, %s: %s; dbengine caches %s", when, side.Role, procStatus(t, side.Daemon.PID()),
				caches[strings.LastIndex(caches, ",")+1:])
		}
	}
	report("ingested")
	time.Sleep(10 * time.Second)
	report("10 s later")
}
