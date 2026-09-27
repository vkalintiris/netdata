// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// ctxCleanupSeed adds to the runR fixture's metadata database a chart of b6child in the context q.x, with a label
// and two dimensions no tier holds, an extra dimension with no data on the chart b6.c0 (context b6.ctx, whose other
// dimensions have data), and cleanup rows for q.x, b6.ctx and q.z, a context no chart has.
const ctxCleanupSeed = `
INSERT INTO chart (chart_id, host_id, type, id, name, family, context, title, unit, plugin, module, priority,
  update_every, chart_type, memory_mode, history_entries)
  SELECT x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', host_id, 'q', 'q.x', 'q.x', 'f', 'q.x', 't', 'u', 'p', '', 1000, 1, 0,
  4, 3600 FROM host WHERE hostname = 'b6child';
INSERT INTO chart_label (chart_id, source_type, label_key, label_value, date_created)
  VALUES (x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', 1, 'k', 'v', 1);
INSERT INTO dimension (dim_id, chart_id, id, name, multiplier, divisor, algorithm)
  VALUES (x'd1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1', x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', 'a', 'a', 1, 1, 0),
         (x'd2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2', x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', 'b', 'b', 1, 1, 0);
INSERT INTO dimension (dim_id, chart_id, id, name, multiplier, divisor, algorithm)
  SELECT x'd3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3', chart_id, 'extra', 'extra', 1, 1, 0 FROM chart
  WHERE type = 'b6' AND id = 'c0' AND host_id = (SELECT host_id FROM host WHERE hostname = 'b6child');
INSERT INTO ctx_metadata_cleanup (host_id, context, date_created)
  SELECT host_id, c, 1 FROM host, (SELECT 'q.x' AS c UNION ALL SELECT 'b6.ctx' UNION ALL SELECT 'q.z')
  WHERE hostname = 'b6child';
`

// ctxCleanupRecords are the context cleanup scan's records.
var ctxCleanupRecords = regexp.MustCompile(`msg="((Verifying the retention|Verified the contexts)[^"]*)"`)

// scanRecords are a daemon's context cleanup records, in order.
func scanRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if m := ctxCleanupRecords.FindStringSubmatch(l); m != nil {
			out = append(out, m[1])
		}
	}
	return out
}

// TestSQLiteCtxCleanup (check `sqlite.ctx-cleanup`): both daemons start on copies of the runR fixture's cache with
// the rows of ctxCleanupSeed, and run the metadata writer's context cleanup scan (about 12 s after the start).
// Compared once both stop: the dimension, chart, chart label and cleanup rows, and the scan's records. With the
// dbengine (as tier 0, or for the children an alloc agent receives) the dimensions no tier holds go, every one when
// the dbengine's files are gone (with the last one of a chart, the chart and its labels); an alloc agent that
// receives alloc children runs no dbengine, and every dimension stays, files or not (C's check is false without the
// dbengine, where a freed dimension's row would go without files, D75.12); either way the cleanup rows of the
// contexts a chart has go and q.z's stays.
func TestSQLiteCtxCleanup(t *testing.T) {
	fx, id := runRParent(t)
	for _, v := range []struct {
		name, mode, children string
		tiers                int
		noFiles              bool
	}{
		{"dbengine", "", "", 3, false},
		{"alloc", "alloc", "", 3, false},
		{"alloc-no-datafiles", "alloc", "", 3, true},
		{"no-dbengine", "alloc", "alloc", 1, false},
		{"no-dbengine-no-datafiles", "alloc", "alloc", 1, true},
	} {
		t.Run(v.name, func(t *testing.T) {
			seed := t.TempDir()
			copyTree(t, filepath.Join(fx, "runR", "cache"), seed)
			execDB(t, filepath.Join(seed, "netdata-meta.db"), ctxCleanupSeed)
			if v.noFiles {
				for _, dir := range []string{"dbengine", "dbengine-tier1", "dbengine-tier2"} {
					if err := os.RemoveAll(filepath.Join(seed, dir)); err != nil {
						t.Fatal(err)
					}
				}
			}
			p := StartPair(t, daemon.Options{StorageTiers: v.tiers, TierRetentionMB: [3]int{25, 25, 25}, DBMode: v.mode,
				StreamMemoryMode: v.children, SeedCache: seed, PulseOff: true, LogsExtra: "    level = debug\n"}, id)
			// the second store job scans, about 12 s after the start; a host the scan skips logs nothing
			deadline := time.Now().Add(40 * time.Second)
			for _, side := range p.Each() {
				for len(scanRecords(t, side.Daemon)) < 2 && time.Now().Before(deadline) {
					time.Sleep(500 * time.Millisecond)
				}
			}
			for _, side := range p.Each() {
				if err := side.Daemon.Stop(); err != nil {
					t.Fatalf("stop %s: %v", side.Role, err)
				}
			}
			args := []string{"--table", "dimension", "--table", "chart", "--table", "chart_label",
				"--table", "ctx_metadata_cleanup", "--mask", "ctx_metadata_cleanup.date_created"}
			var dumps, records [2]string
			for i, side := range p.Each() {
				dumps[i] = dumpDB(t, filepath.Join(side.Daemon.Opts.RunDir, "cache", "netdata-meta.db"), args...)
				records[i] = strings.Join(scanRecords(t, side.Daemon), "\n")
			}
			if dumps[0] != dumps[1] {
				t.Errorf("tables differ:\noracle:\n%s\ncandidate:\n%s", dumps[0], dumps[1])
			}
			if records[0] != records[1] {
				t.Errorf("scan records differ:\noracle:\n%s\ncandidate:\n%s", records[0], records[1])
			}
			t.Logf("oracle scan records:\n%s", records[0])
		})
	}
}

// cyclesSeed adds orphans to the runR fixture's metadata database: two dimensions no tier holds on a chart of b6child
// (with a label), a chart of b6child without dimensions, and a label of a chart that does not exist.
const cyclesSeed = `
INSERT INTO chart (chart_id, host_id, type, id, name, family, context, title, unit, plugin, module, priority,
  update_every, chart_type, memory_mode, history_entries)
  SELECT x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', host_id, 'q', 'q.x', 'q.x', 'f', 'q.x', 't', 'u', 'p', '', 1000, 1, 0,
  4, 3600 FROM host WHERE hostname = 'b6child';
INSERT INTO chart (chart_id, host_id, type, id, name, family, context, title, unit, plugin, module, priority,
  update_every, chart_type, memory_mode, history_entries)
  SELECT x'c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2', host_id, 'q', 'q.y', 'q.y', 'f', 'q.y', 't', 'u', 'p', '', 1000, 1, 0,
  4, 3600 FROM host WHERE hostname = 'b6child';
INSERT INTO chart_label (chart_id, source_type, label_key, label_value, date_created)
  VALUES (x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', 1, 'k', 'v', 1), (x'c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3', 1, 'k', 'v', 1);
INSERT INTO dimension (dim_id, chart_id, id, name, multiplier, divisor, algorithm)
  VALUES (x'd1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1', x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', 'a', 'a', 1, 1, 0),
         (x'd2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2', x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', 'b', 'b', 1, 1, 0);
`

// cycleRecords are the dimension, chart and chart-label cycles' records.
var cycleRecords = regexp.MustCompile(`msg="((Dimension|Chart|Chart label) metadata check [^"]*|Checking (dimensions|charts|chart labels) [^"]*|(Dimensions|Charts|Chart labels) checked [^"]*)"`)

func cycleLines(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if m := cycleRecords.FindStringSubmatch(l); m != nil {
			out = append(out, m[1])
		}
	}
	return out
}

// TestSQLiteMetadataCycles (check `sqlite.metadata-cycles`, about 32 minutes: set PARITY_LONG=1): both daemons on
// copies of the runR cache with cyclesSeed's orphans run the metadata writer's dimension, chart and chart-label cycles
// 1800 s after their first store job. Compared once the chart cycle completed on both (the label cycle's completion
// comes an hour later): the dimension, chart and chart-label rows and the cycles' records in order.
func TestSQLiteMetadataCycles(t *testing.T) {
	if os.Getenv("PARITY_LONG") == "" {
		t.Skip("PARITY_LONG unset (the cycles start 1800 s after the start)")
	}
	fx, id := runRParent(t)
	seed := t.TempDir()
	copyTree(t, filepath.Join(fx, "runR", "cache"), seed)
	execDB(t, filepath.Join(seed, "netdata-meta.db"), cyclesSeed)
	p := StartPair(t, daemon.Options{StorageTiers: 3, TierRetentionMB: [3]int{25, 25, 25}, SeedCache: seed,
		PulseOff: true, LogsExtra: "    level = debug\n"}, id)
	deadline := time.Now().Add(40 * time.Minute)
	for _, side := range p.Each() {
		for !strings.Contains(strings.Join(cycleLines(t, side.Daemon), "\n"), "Chart metadata check completed") {
			if time.Now().After(deadline) {
				t.Fatalf("%s: the chart cycle did not complete: %q", side.Role, cycleLines(t, side.Daemon))
			}
			time.Sleep(5 * time.Second)
		}
	}
	for _, side := range p.Each() {
		if err := side.Daemon.Stop(); err != nil {
			t.Fatalf("stop %s: %v", side.Role, err)
		}
	}
	args := []string{"--table", "dimension", "--table", "chart", "--table", "chart_label", "--sort", "chart_label"}
	var dumps, records [2]string
	for i, side := range p.Each() {
		dumps[i] = dumpDB(t, filepath.Join(side.Daemon.Opts.RunDir, "cache", "netdata-meta.db"), args...)
		records[i] = strings.Join(cycleLines(t, side.Daemon), "\n")
	}
	if dumps[0] != dumps[1] {
		t.Errorf("tables differ:\noracle:\n%s\ncandidate:\n%s", dumps[0], dumps[1])
	}
	if records[0] != records[1] {
		t.Errorf("cycle records differ:\noracle:\n%s\ncandidate:\n%s", records[0], records[1])
	}
	t.Logf("oracle cycle records:\n%s", records[0])
}
