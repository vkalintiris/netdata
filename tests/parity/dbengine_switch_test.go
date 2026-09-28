// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// switchLegs is how many times the switching side changes binary: C, Rust, C, Rust.
const switchLegs = 4

// TestDbengineSwitchLoop (check `dbengine.switch-loop`, S7a, D93): side A runs C only, side B alternates C and the
// Rust agent on its own cache, both fed the same S3 child history in four chunks, one per leg. After each leg B
// answers as A (contexts, the S3 windows up to the chunk's end) and keeps its dimension rows (no new UUIDs); after
// each stop B's v2 files normalize. What differs between the sides is only which binary wrote each part of B's files.
func TestDbengineSwitchLoop(t *testing.T) {
	opts := daemon.Options{StorageTiers: 1, TierRetentionMB: [3]int{25}, PulseOff: true,
		LogsExtra: "    level = debug\n"}
	bins := binaries(t)
	end := time.Now().Unix() - 3600
	g := s3gen{start: end - s3Span + 1}
	chunk := int64(s3Span / switchLegs)
	// each leg's run directories go with its subtest: the caches it leaves are kept here
	keep := t.TempDir()
	var caches [2]string
	var dims string
	for leg := 0; leg < switchLegs; leg++ {
		switching := bins[leg%2]
		name := []string{"c", "rust"}[leg%2]
		from, to := g.start+int64(leg)*chunk, g.start+int64(leg+1)*chunk-1
		if leg == switchLegs-1 {
			to = end
		}
		ok := t.Run(fmt.Sprintf("leg%d-%s", leg, name), func(t *testing.T) {
			roles := [2]Role{Role(fmt.Sprintf("leg%d-a", leg)), Role(fmt.Sprintf("leg%d-b-%s", leg, name))}
			var p *Pair
			if leg == 0 {
				p = startPair(t, opts, parentIdentity, [2]string{bins[0], switching}, [2]string{}, roles)
			} else {
				p = startPair(t, opts, parentIdentity, [2]string{bins[0], switching}, caches, roles)
			}
			g.streamBoth(t, p, from, to)
			rules := dbengineWriteRules(false)
			rules.Settle = 15 * time.Second
			compareGetWith(t, p, "/host/s3child/api/v1/contexts?options=full", rules)
			g.compareWindows(t, p, to, false)
			for _, side := range p.Each() {
				if err := side.Daemon.Stop(); err != nil {
					t.Fatalf("stop %s: %v", side.Role, err)
				}
			}
			for i, side := range p.Each() {
				caches[i] = filepath.Join(keep, fmt.Sprintf("leg%d-%d", leg, i), "cache")
				copyTree(t, filepath.Join(side.Daemon.Opts.RunDir, "cache"), caches[i])
			}
			if code, out := inspect(t, "--normalize-v2", caches[1]); code != 0 {
				t.Errorf("B: dbengine-inspect --normalize-v2: exit %d\n%s", code, out)
			}
			// the child's dimensions keep their rows from the first leg on
			got := dumpDB(t, filepath.Join(caches[1], "netdata-meta.db"), "--table", "dimension")
			if leg == 0 {
				dims = got
				if n := strings.Count(dims, "\n"); n < s3Charts*s3Dims {
					t.Fatalf("B: %d dimension rows after the first leg, want at least %d", n, s3Charts*s3Dims)
				}
			} else if got != dims {
				t.Errorf("B: the dimension rows changed:\n%s", firstDifference([]byte(dims), []byte(got)))
			}
		})
		if !ok {
			return
		}
	}
}
