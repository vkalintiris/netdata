// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/json"
	"fmt"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// killChild is the early case's child: two charts of three dimensions, a few seconds of live points.
var killChild = stream.HostInfo{Hostname: "killchild", MachineGUID: "b6b6b6b6-1111-4111-8111-000000000009"}

// killPair ends both daemons with SIGKILL and returns their caches.
func killPair(t *testing.T, p *Pair) [2]string {
	t.Helper()
	var caches [2]string
	for i, side := range p.Each() {
		if err := side.Daemon.Kill(); err != nil {
			t.Fatalf("kill %s: %v", side.Role, err)
		}
		caches[i] = filepath.Join(side.Daemon.Opts.RunDir, "cache")
	}
	return caches
}

// TestDbengineKill9 (check `dbengine.kill9`, S7a, D93): the agents are killed with SIGKILL, and each killed cache is
// then started by C and by the Rust agent, each on its own copy: the same replay decisions and records, the same
// answers, the same files after a clean stop. The two killed caches are never compared with each other: what each
// agent flushed before the kill is its own timing (D65.1).
//   - `hot-pages`: killed once the S3 child's history is ingested, its last pages and the dirty pages short of an
//     extent still in memory.
//   - `early`: killed a second after a new child's first points, most likely before METASYNC's first store (6 s after
//     its start, then every 6 s); either way both agents start on the same files.
func TestDbengineKill9(t *testing.T) {
	opts := daemon.Options{StorageTiers: 1, TierRetentionMB: [3]int{25}, PulseOff: true,
		LogsExtra: "    level = debug\n"}
	replay := func(t *testing.T, killed [2]string, compare func(t *testing.T, r *Pair)) {
		for i, from := range []string{"oracle-killed", "candidate-killed"} {
			t.Run(from, func(t *testing.T) {
				r := startPair(t, opts, parentIdentity, binaries(t), [2]string{killed[i], killed[i]},
					[2]Role{Role(from + "-c"), Role(from + "-rust")})
				compare(t, r)
				for _, side := range r.Each() {
					if err := side.Daemon.Stop(); err != nil {
						t.Fatalf("stop %s: %v", side.Role, err)
					}
				}
				compareLogFilesWith(t, r, writeLogMasks, "daemon.log")
				caches := [2]string{filepath.Join(r.Oracle.Opts.RunDir, "cache"),
					filepath.Join(r.Candidate.Opts.RunDir, "cache")}
				if o, c := cacheFiles(t, caches[0]), cacheFiles(t, caches[1]); strings.Join(o, " ") != strings.Join(c, " ") {
					t.Errorf("files: oracle %v, candidate %v", o, c)
				}
				if code, out := inspect(t, "--normalize-v2", caches[1]); code != 0 {
					t.Errorf("candidate: dbengine-inspect --normalize-v2: exit %d\n%s", code, out)
				}
			})
		}
	}
	t.Run("hot-pages", func(t *testing.T) {
		p := StartPair(t, opts, parentIdentity)
		end := time.Now().Unix() - 3600
		g := s3gen{start: end - s3Span + 1}
		g.streamBoth(t, p, g.start, end)
		replay(t, killPair(t, p), func(t *testing.T, r *Pair) {
			rules := dbengineWriteRules(false)
			rules.Settle = 15 * time.Second
			compareGetWith(t, r, "/host/s3child/api/v1/contexts?options=full", rules)
			g.compareWindows(t, r, end, false)
		})
	})
	t.Run("early", func(t *testing.T) {
		p := StartPair(t, opts, parentIdentity)
		now := time.Now().Unix()
		g := childGen{host: killChild, prefix: "kill.", charts: 2, dims: 3,
			context: func(c int) string { return fmt.Sprintf("kill.ctx%d", c) },
			skips:   func(int, int64) bool { return false },
			point: func(c, d int, t int64) (string, string) {
				return strconv.Itoa(c*10 + d + int(t%7)), stream.FlagNotAnomalous
			}}
		for _, side := range p.Each() {
			if err := g.stream(side.Daemon, now-3, now); err != nil {
				t.Fatalf("%s: streaming: %v", side.Role, err)
			}
		}
		time.Sleep(time.Second)
		killed := killPair(t, p)
		for i, side := range p.Each() {
			hosts := dumpDB(t, filepath.Join(killed[i], "netdata-meta.db"), "--table", "host")
			t.Logf("%s: the child's host row was stored before the kill: %v", side.Role,
				strings.Contains(hosts, killChild.MachineGUID))
		}
		replay(t, killed, func(t *testing.T, r *Pair) {
			// the hosts the agent knows: /api/v1/info's host lists (the rest of it is not ported, D84.2)
			var hosts [2]string
			for i, side := range r.Each() {
				b, err := rawExchange(side.Daemon.Addr, []byte("GET /api/v1/info HTTP/1.1\r\n\r\n"), 10*time.Second)
				if err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				var doc map[string]any
				if err := json.Unmarshal(httpBody(b), &doc); err != nil {
					t.Fatalf("%s: /api/v1/info: %v", side.Role, err)
				}
				hosts[i] = fmt.Sprint(doc["hosts-available"], doc["mirrored_hosts"])
			}
			if hosts[0] != hosts[1] {
				t.Errorf("hosts: oracle %s, candidate %s", hosts[0], hosts[1])
			}
			compareGet(t, r, "/host/"+killChild.Hostname+"/api/v1/contexts?options=full")
			win := fmt.Sprintf("after=%d&before=%d", now-60, now+60)
			compareGet(t, r, "/host/"+killChild.Hostname+"/api/v3/data?contexts=kill.*&"+win+"&points=10")
		})
	})
}
