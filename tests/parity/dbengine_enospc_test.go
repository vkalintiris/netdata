// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// mountTier0 mounts a tmpfs of `size` over a run directory's tier-0 directory with sudo (D93.3: a scratch
// directory only), unmounted once the test's daemons stop. The cases that need it run with PARITY_SUDO=1.
func mountTier0(t *testing.T, runDir, size string) string {
	t.Helper()
	if os.Getenv("PARITY_SUDO") != "1" {
		t.Skip("PARITY_SUDO unset: the full-disk cases mount a tmpfs with sudo")
	}
	dir := filepath.Join(runDir, "cache", "dbengine")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	opts := fmt.Sprintf("size=%s,uid=%d,gid=%d,mode=0755", size, os.Getuid(), os.Getgid())
	if out, err := exec.Command("sudo", "-n", "mount", "-t", "tmpfs", "-o", opts, "tmpfs", dir).CombinedOutput(); err != nil {
		t.Fatalf("mount a tmpfs on %s: %v: %s", dir, err, out)
	}
	// registered before the daemon's stop, so it runs after it
	t.Cleanup(func() {
		if out, err := exec.Command("sudo", "-n", "umount", dir).CombinedOutput(); err != nil {
			t.Errorf("umount %s: %v: %s", dir, err, out)
		}
	})
	return dir
}

// fillDir writes a filler file into dir until its filesystem is full (the engine ignores a name it does not know).
func fillDir(t *testing.T, dir string) {
	t.Helper()
	f, err := os.Create(filepath.Join(dir, "filler"))
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	block := make([]byte, 4096)
	for {
		if _, err := f.Write(block); err != nil {
			if !errors.Is(err, syscall.ENOSPC) {
				t.Fatalf("fill %s: %v", dir, err)
			}
			break
		}
	}
	// what a partial block left
	for {
		if _, err := f.Write(block[:1]); err != nil {
			break
		}
	}
}

// pathsRe finds a record's paths; with numbersRe it normalizes a record to its kind.
var pathsRe = regexp.MustCompile(`"/[^"]*"|/[^ ,"]*`)

// engineErrorKinds are a daemon's dbengine error and warning records as a sorted set of kinds (numbers and paths
// replaced), C's protected-access record left out (the Rust agent reads without mmap faults, D93.5).
func engineErrorKinds(t *testing.T, d *daemon.Daemon) string {
	t.Helper()
	kinds := map[string]bool{}
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if !strings.Contains(l, "DBENGINE") || !(strings.Contains(l, "level=error") || strings.Contains(l, "level=warning")) ||
			strings.Contains(l, "Protected access fault") {
			continue
		}
		msg := l[strings.Index(l, "msg="):]
		kinds[numbersRe.ReplaceAllString(pathsRe.ReplaceAllString(msg, "<path>"), "N")] = true
	}
	var out []string
	for k := range kinds {
		out = append(out, k)
	}
	slices.Sort(out)
	return strings.Join(out, "\n")
}

// TestDbengineENOSPC (check `dbengine.enospc`, S7a, D93.3): tier 0 on a full or unwritable directory.
//   - `eacces` (no sudo): the runR cache with tier 0's directory read-only: the new pair cannot be created.
//   - `startup-full`: the runR cache on a tmpfs filled to the last byte: the new pair and the last journal's index
//     cannot be written.
//   - `runtime-full`: a fresh cache whose tier-0 tmpfs fills while the S3 child's history is ingested. Where the disk
//     fills is each agent's own page composition (D65.1), so the sides compare by the kinds of records they log and by
//     answering; then each cache left is started by both agents (replayEach).
//
// The startup cases compare the answers about the archived child and the whole daemon log; every agent keeps
// answering.
func TestDbengineENOSPC(t *testing.T) {
	fx, id := runRParent(t)
	seed := filepath.Join(fx, "runR", "cache")
	opts := daemon.Options{StorageTiers: 3, TierRetentionMB: [3]int{25, 25, 25}, PulseOff: true,
		LogsExtra: "    level = debug\n"}
	// C's record of the SIGBUS its index build takes on a full tmpfs (with the fault address): the Rust agent writes
	// the index without a mapping and has no such record (D93.5)
	protectedAccess := []logMask{{regexp.MustCompile(`^.*msg="Protected access fault in .*$`), ""}}
	startupChecks := func(t *testing.T, p *Pair) {
		compareGet(t, p, "/host/b6child/api/v1/contexts")
		win := fmt.Sprintf("after=%d&before=%d", fixtureStart, fixtureEnd)
		compareGet(t, p, "/host/b6child/api/v1/data?chart=b6.c2&"+win+"&points=30&tier=0&options=jsonwrap")
		compareGet(t, p, "/api/v3/data?scope_nodes=*&scope_contexts=*&"+win+"&points=10&tier=0")
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		compareLogFilesWith(t, p, protectedAccess, "daemon.log")
		kinds := engineErrorKinds(t, p.Oracle)
		if kinds == "" {
			t.Errorf("the oracle logged no dbengine error: the fault did not bite")
		}
		t.Logf("the oracle's dbengine errors:\n%s", kinds)
	}
	t.Run("eacces", func(t *testing.T) {
		p := startPairWith(t, opts, id, binaries(t), [2]string{}, [2]Role{"eacces-oracle", "eacces-candidate"},
			func(t *testing.T, runDir string) {
				copyTree(t, seed, filepath.Join(runDir, "cache"))
				tier0 := filepath.Join(runDir, "cache", "dbengine")
				if err := os.Chmod(tier0, 0o555); err != nil {
					t.Fatal(err)
				}
				t.Cleanup(func() { _ = os.Chmod(tier0, 0o755) })
			})
		startupChecks(t, p)
	})
	t.Run("startup-full", func(t *testing.T) {
		p := startPairWith(t, opts, id, binaries(t), [2]string{}, [2]Role{"full-oracle", "full-candidate"},
			func(t *testing.T, runDir string) {
				tier0 := mountTier0(t, runDir, "4m")
				copyTree(t, seed, filepath.Join(runDir, "cache"))
				fillDir(t, tier0)
			})
		startupChecks(t, p)
	})
	t.Run("runtime-full", func(t *testing.T) {
		o := daemon.Options{StorageTiers: 1, TierRetentionMB: [3]int{25}, PulseOff: true,
			LogsExtra: "    level = debug\n"}
		p := startPairWith(t, o, parentIdentity, binaries(t), [2]string{}, [2]Role{"runtime-oracle", "runtime-candidate"},
			func(t *testing.T, runDir string) { mountTier0(t, runDir, "640k") })
		end := time.Now().Unix() - 3600
		g := s3gen{start: end - s3Span + 1}
		g.streamBoth(t, p, g.start, end)
		for _, side := range p.Each() {
			if _, err := rawExchange(side.Daemon.Addr, []byte("GET /api/v1/info HTTP/1.1\r\n\r\n"), 10*time.Second); err != nil {
				t.Errorf("%s does not answer after the disk filled: %v", side.Role, err)
			}
		}
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		o1, c1 := engineErrorKinds(t, p.Oracle), engineErrorKinds(t, p.Candidate)
		if o1 == "" {
			t.Errorf("the oracle logged no dbengine error: the disk did not fill")
		}
		if o1 != c1 {
			t.Errorf("dbengine error kinds differ\noracle:\n%s\ncandidate:\n%s", o1, c1)
		}
		t.Logf("the oracle's dbengine errors:\n%s", o1)
		// the caches left, off the tmpfs, started by both agents
		var left [2]string
		for i, side := range p.Each() {
			left[i] = filepath.Join(t.TempDir(), "cache")
			copyTree(t, filepath.Join(side.Daemon.Opts.RunDir, "cache"), left[i])
		}
		// the extent the full disk cut short stays in the data file past the journal's end: no v2 normalization
		replayEach(t, o, left, [2]string{"oracle-full", "candidate-full"}, false, func(t *testing.T, r *Pair) {
			rules := dbengineWriteRules(false)
			rules.Settle = 15 * time.Second
			compareGetWith(t, r, "/host/s3child/api/v1/contexts?options=full", rules)
			g.compareWindows(t, r, end, false)
		})
	})
}
