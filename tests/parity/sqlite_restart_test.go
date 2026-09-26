// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/json"
	"os"
	"path/filepath"
	"regexp"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// agentTimes hides the stream path entries' times, and the parent's own startup and shutdown medians, which come
// from the agent event log of each agent's own runs.
var agentTimes = regexp.MustCompile(`("since":)\d+(,\s*"first_time_t":)\d+(?:,\s*"start_time":\d+,\s*"shutdown_time":\d+)?`)

// startOn starts a binary on a copy of a cache directory.
func startOn(t *testing.T, binary string, role Role, cache string, opts daemon.Options) *daemon.Daemon {
	t.Helper()
	id := parentIdentity
	opts.Binary, opts.RunDir, opts.Identity, opts.SeedCache = binary, runDir(t, role), &id, cache
	d, err := daemon.Start(opts)
	if err != nil {
		t.Fatalf("%s: %v", role, err)
	}
	t.Cleanup(func() { _ = d.Stop() })
	return d
}

// TestSQLiteRestart restarts both daemons after a child streamed to them, each on the cache it left (check
// `sqlite.restart`): the child is an archived host of the second run, with the same info, labels, contexts, stream
// path and records as C after C. Then C starts on the Rust agent's cache next to C on its own: C after the Rust agent
// lists the Rust agent's child as it lists its own.
func TestSQLiteRestart(t *testing.T) {
	opts := daemon.Options{DBMode: "alloc", StorageTiers: 1, StreamMemoryMode: "alloc", LogsExtra: "    level = debug\n"}
	first := StartPair(t, opts, parentIdentity)
	for _, side := range first.Each() {
		writerChild(t, side.Daemon)
	}
	// the metadata writer's first job runs 6 s after METASYNC starts
	time.Sleep(8 * time.Second)
	var caches [2]string
	for i, side := range first.Each() {
		if err := side.Daemon.Stop(); err != nil {
			t.Fatalf("stop %s: %v", side.Role, err)
		}
		caches[i] = filepath.Join(side.Daemon.Opts.RunDir, "cache")
	}
	oracle, candidate := os.Getenv("PARITY_ORACLE"), os.Getenv("PARITY_CANDIDATE")
	t.Run("restart", func(t *testing.T) {
		p := &Pair{
			Oracle:    startOn(t, oracle, Role("oracle"), caches[0], opts),
			Candidate: startOn(t, candidate, Role("candidate"), caches[1], opts),
		}
		compareArchivedTimes(t, p, agentTimes, childHost.Hostname)
	})
	t.Run("c-after-rust", func(t *testing.T) {
		p := &Pair{
			Oracle:    startOn(t, oracle, Role("c-after-c"), caches[0], opts),
			Candidate: startOn(t, oracle, Role("c-after-rust"), caches[1], opts),
		}
		compareArchivedTimes(t, p, agentTimes, childHost.Hostname)
		// the child's labels as C stored them and as the Rust agent did
		for _, side := range p.Each() {
			_, labels := infoIdentity(t, side.Daemon.Addr, "/host/"+childHost.Hostname+"/api/v1/info", false)
			if b, _ := json.Marshal(labels); len(labels) == 0 {
				t.Errorf("%s: no child labels: %s", side.Role, b)
			}
		}
	})
}
